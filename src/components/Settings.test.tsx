// Page Settings face au simulateur : textes, enregistrement de chaque
// réglage, « Saved » après succès seulement, erreurs, Reset et relecture.

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
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { setLanguage } from "@/i18n";
import { validateSettingsSearch } from "@/routes/settings";
import { fakeBackend, SERVER_URL } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });

const CONTEXT_DESCRIPTION =
  "Context length determines how much of your conversation local LLMs can remember and use to generate responses.";
const RESET_ERROR =
  "ChatonLM could not reset every setting. Check the settings above and try again.";

function setup(
  settings: Partial<Settings> = {},
  path = "/settings",
  prepare?: (backend: ReturnType<typeof fakeBackend>) => void,
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  Object.assign(backend.settings, settings);
  // Comme le moteur : adresse réglée, sinon celle de l'environnement.
  backend.settings.effectiveServerUrl =
    backend.settings.serverUrl || SERVER_URL;
  prepare?.(backend);
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** update_settings retenu jusqu'à `release()` (réponse) ou `reject()`. */
function holdUpdates(
  simulator: ReturnType<typeof installSimulator>,
  backend: ReturnType<typeof fakeBackend>,
) {
  const held: {
    patch: SettingsPatch;
    release: () => void;
    reject: (error: unknown) => void;
  }[] = [];
  simulator.handle("update_settings", (args) => {
    const patch = args.patch as SettingsPatch;
    return new Promise((resolve, reject) => {
      held.push({
        patch,
        release: () => {
          Object.assign(backend.settings, patch);
          backend.settings.effectiveServerUrl =
            backend.settings.serverUrl || SERVER_URL;
          resolve({ ...backend.settings });
        },
        reject,
      });
    });
  });
  return held;
}

const patches = (simulator: ReturnType<typeof installSimulator>) =>
  simulator.callsTo("update_settings").map((args) => args.patch);

const saved = () => screen.queryByText("Saved");

describe("Settings", () => {
  it("shows one card with the three settings", async () => {
    setup({ contextLength: 8192, theme: "dark", serverUrl: "" });
    await screen.findByTestId("settings");
    expect(screen.getByText("Server address")).toBeTruthy();
    expect(screen.getByText("Context length")).toBeTruthy();
    expect(screen.getByText(CONTEXT_DESCRIPTION)).toBeTruthy();
    expect(screen.getByText("Theme")).toBeTruthy();
    expect(
      screen.getByText(
        "Leave empty to use the OLLAMA_HOST environment variable, or http://localhost:11434 if it isn't set.",
      ),
    ).toBeTruthy();
    // Champ d'adresse relié à son libellé, adresse utilisée en indication.
    const address = screen.getByLabelText<HTMLInputElement>("Server address");
    expect(address.value).toBe("");
    expect(address.placeholder).toBe("http://127.0.0.1:11434");
    // Curseur : 8 crans, valeur enregistrée.
    const slider = screen.getByRole("slider", { name: "Context length" });
    expect(slider.getAttribute("aria-valuetext")).toBe("8k");
    for (const label of ["Default", "4k", "8k", "16k", "32k", "64k", "128k"]) {
      expect(screen.getByText(label)).toBeTruthy();
    }
    expect(screen.getByText("256k")).toBeTruthy();
    // Thème : trois choix, celui enregistré coché.
    const group = screen.getByRole("radiogroup", { name: "Theme" });
    expect(group).toBeTruthy();
    expect(
      within(group).getByRole<HTMLInputElement>("radio", { name: "Dark" })
        .checked,
    ).toBe(true);
    expect(
      within(group).getByRole<HTMLInputElement>("radio", { name: "System" })
        .checked,
    ).toBe(false);
    expect(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(saved()).toBeNull();
  });

  it("chooses the language first: System, then each language written in itself", async () => {
    const { simulator } = setup();
    const group = await screen.findByRole("radiogroup", { name: "Language" });
    const choices = within(group).getAllByRole<HTMLInputElement>("radio");
    expect(choices.map((choice) => choice.labels?.[0].textContent)).toEqual([
      "System",
      "English",
      "Español",
      "Français",
    ]);
    expect(choices[0].checked).toBe(true);
    // Première ligne de la carte.
    const rows = screen.getAllByRole("radiogroup");
    expect(rows[0]).toBe(group);
    // Libellé dans sa langue, pour les lecteurs d'écran.
    expect(within(group).getByText("Español").getAttribute("lang")).toBe("es");

    // Toute la page change aussitôt, sans rechargement.
    await userEvent.click(
      within(group).getByRole("radio", { name: "Español" }),
    );
    await screen.findByRole("radiogroup", { name: "Idioma" });
    expect(patches(simulator)).toEqual([{ language: "es" }]);
    expect(document.documentElement.lang).toBe("es");
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: "Español" }).checked,
    ).toBe(true);
    // Les noms des langues ne changent pas ; « System » si (celui du thème
    // aussi : choix de la langue seulement).
    await userEvent.click(
      within(screen.getByRole("radiogroup", { name: "Idioma" })).getByRole(
        "radio",
        { name: "Sistema" },
      ),
    );
    await screen.findByRole("radiogroup", { name: "Language" });
    expect(document.documentElement.lang).toBe("en");
  });

  it("saves the theme and shows Saved only once it is saved", async () => {
    const { simulator, backend } = setup();
    const held = holdUpdates(simulator, backend);
    const dark = await screen.findByRole<HTMLInputElement>("radio", {
      name: "Dark",
    });
    await userEvent.click(dark);
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].patch).toEqual({ theme: "dark" });
    // Pas encore enregistré : ni badge, ni changement affiché.
    expect(saved()).toBeNull();
    expect(dark.checked).toBe(false);
    act(() => held[0].release());
    await screen.findByText("Saved");
    await waitFor(() => expect(dark.checked).toBe(true));
    expect(screen.getByRole("status").textContent).toBe("Saved");
  });

  it("hides Saved after 3 seconds", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      setup();
      await userEvent.click(
        await screen.findByRole("radio", { name: "Light" }),
      );
      await screen.findByText("Saved");
      act(() => vi.advanceTimersByTime(2900));
      expect(saved()).not.toBeNull();
      act(() => vi.advanceTimersByTime(200));
      expect(saved()).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("saves the context length from the slider", async () => {
    const { simulator } = setup({ contextLength: 0 });
    const slider = await screen.findByRole("slider", {
      name: "Context length",
    });
    expect(slider.getAttribute("aria-valuetext")).toBe("Default");
    act(() => slider.focus());
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    await screen.findByText("Saved");
    expect(patches(simulator)).toEqual([{ contextLength: 4096 }]);
    // La pastille est un autre élément sur chaque cran : relue.
    await waitFor(() =>
      expect(
        screen
          .getByRole("slider", { name: "Context length" })
          .getAttribute("aria-valuetext"),
      ).toBe("4k"),
    );
  });

  it("shows an error, and no Saved, when a setting isn't saved", async () => {
    const { backend } = setup();
    backend.settingsErrors.update = { kind: "database", message: "disk full" };
    const dark = await screen.findByRole<HTMLInputElement>("radio", {
      name: "Dark",
    });
    await userEvent.click(dark);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: disk full",
    );
    expect(saved()).toBeNull();
    expect(dark.checked).toBe(false);
    // Le changement suivant efface l'erreur.
    backend.settingsErrors.update = null;
    await userEvent.click(screen.getByRole("radio", { name: "Light" }));
    await screen.findByText("Saved");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("saves the server address, and shows why it was refused", async () => {
    const { simulator } = setup();
    const address =
      await screen.findByLabelText<HTMLInputElement>("Server address");
    const save = screen.getByRole<HTMLButtonElement>("button", {
      name: "Save",
    });
    expect(save.disabled).toBe(true);
    await userEvent.type(address, "ftp://example.com{Enter}");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/invalid server address/);
    expect(saved()).toBeNull();
    await userEvent.clear(address);
    await userEvent.type(address, "  http://192.168.1.20:11434 ");
    await userEvent.click(save);
    await screen.findByText("Saved");
    expect(patches(simulator).at(-1)).toEqual({
      serverUrl: "http://192.168.1.20:11434",
    });
    // Champ remonté avec la valeur enregistrée : relu.
    await waitFor(() => {
      const field = screen.getByLabelText<HTMLInputElement>("Server address");
      expect(field.value).toBe("http://192.168.1.20:11434");
      expect(field.placeholder).toBe("http://192.168.1.20:11434");
    });
  });

  it("resets the language, address, context length, theme and reading aloud, not the model", async () => {
    const { simulator, backend } = setup({
      serverUrl: "http://10.0.0.2:11434",
      contextLength: 32768,
      theme: "dark",
      selectedModel: "qwen3.5:0.8b",
      language: "en",
      autoRead: true,
    });
    const held = holdUpdates(simulator, backend);
    const address =
      await screen.findByLabelText<HTMLInputElement>("Server address");
    // Brouillon non enregistré : abandonné par le Reset.
    await userEvent.type(address, "/draft");
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].patch).toEqual({
      language: "system",
      serverUrl: "",
      contextLength: 0,
      theme: "system",
      autoRead: false,
    });
    const button = screen.getByRole<HTMLButtonElement>("button", {
      name: "Resetting…",
    });
    expect(button.disabled).toBe(true);
    const fieldset = screen
      .getByTestId("settings")
      .querySelector("fieldset") as HTMLFieldSetElement;
    expect(fieldset.disabled).toBe(true);
    expect(fieldset.getAttribute("aria-busy")).toBe("true");
    expect(
      screen.getByRole("slider", { name: "Context length" }).tabIndex,
    ).toBe(-1);
    act(() => held[0].release());
    await screen.findByText("Saved");
    expect(fieldset.disabled).toBe(false);
    expect(screen.getByRole("button", { name: "Reset to defaults" }));
    await waitFor(() =>
      expect(
        screen.getByLabelText<HTMLInputElement>("Server address").value,
      ).toBe(""),
    );
    await waitFor(() => {
      expect(
        screen
          .getByRole("slider", { name: "Context length" })
          .getAttribute("aria-valuetext"),
      ).toBe("Default");
      for (const group of ["Language", "Theme"]) {
        expect(
          within(
            screen.getByRole("radiogroup", { name: group }),
          ).getByRole<HTMLInputElement>("radio", { name: "System" }).checked,
        ).toBe(true);
      }
    });
    expect(
      screen.getByLabelText<HTMLInputElement>("Server address").placeholder,
    ).toBe(SERVER_URL);
    expect(backend.settings.selectedModel).toBe("qwen3.5:0.8b");
    expect(backend.settings.autoRead).toBe(false);
  });

  it("drops an address draft on Reset even if the address doesn't change", async () => {
    setup({ serverUrl: "" });
    const address =
      await screen.findByLabelText<HTMLInputElement>("Server address");
    await userEvent.type(address, "draft");
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    await waitFor(() =>
      expect(
        screen.getByLabelText<HTMLInputElement>("Server address").value,
      ).toBe(""),
    );
  });

  it("hides Saved while resetting", async () => {
    const { simulator, backend } = setup();
    await userEvent.click(await screen.findByRole("radio", { name: "Dark" }));
    await screen.findByText("Saved");
    const held = holdUpdates(simulator, backend);
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByRole("button", { name: "Resetting…" });
    expect(saved()).toBeNull();
    act(() => held[0].release());
    await screen.findByText("Saved");
  });

  it("explains a failed reset", async () => {
    const { simulator, backend } = setup({ theme: "dark" });
    const held = holdUpdates(simulator, backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Reset to defaults" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    act(() => held[0].reject({ kind: "database", message: "locked" }));
    expect((await screen.findByRole("alert")).textContent).toBe(RESET_ERROR);
    consoleError.mockRestore();
    expect(saved()).toBeNull();
    expect(
      screen.getByRole<HTMLButtonElement>("button", {
        name: "Reset to defaults",
      }).disabled,
    ).toBe(false);
    expect(
      screen.getByRole<HTMLInputElement>("radio", { name: "Dark" }).checked,
    ).toBe(true);
  });

  it("says when the settings can't be loaded", async () => {
    setup({}, "/settings", (backend) => {
      backend.settingsErrors.get = { kind: "database", message: "locked" };
    });
    expect(await screen.findByText("Failed to load settings")).toBeTruthy();
    expect(screen.queryByTestId("settings")).toBeNull();
  });

  it("reads the settings again when the page opens", async () => {
    const { backend, app } = setup({}, "/c/new");
    await screen.findByLabelText("Message");
    backend.settingsErrors.get = { kind: "database", message: "locked" };
    await act(() => app.router.navigate({ to: "/settings" }));
    expect(await screen.findByText("Failed to load settings")).toBeTruthy();
  });

  it("keeps the chosen step while it is being saved, and goes on from it", async () => {
    const { simulator, backend } = setup({ contextLength: 0 });
    const held = holdUpdates(simulator, backend);
    const slider = () => screen.getByRole("slider", { name: "Context length" });
    await screen.findByRole("slider", { name: "Context length" });
    act(() => slider().focus());
    fireEvent.keyDown(slider(), { key: "ArrowRight" });
    fireEvent.keyDown(slider(), { key: "ArrowRight" });
    await waitFor(() => expect(held).toHaveLength(1));
    // En attente : la pastille reste sur le dernier cran choisi.
    expect(slider().getAttribute("aria-valuetext")).toBe("8k");
    act(() => held[0].release());
    await waitFor(() => expect(held).toHaveLength(2));
    expect(slider().getAttribute("aria-valuetext")).toBe("8k");
    act(() => held[1].release());
    await screen.findByText("Saved");
    expect(held.map((h) => h.patch)).toEqual([
      { contextLength: 4096 },
      { contextLength: 8192 },
    ]);
    await waitFor(() =>
      expect(slider().getAttribute("aria-valuetext")).toBe("8k"),
    );
    expect(backend.settings.contextLength).toBe(8192);
  });

  it("goes back to the saved step when a change fails", async () => {
    const { backend } = setup({ contextLength: 4096 });
    backend.settingsErrors.update = { kind: "database", message: "disk full" };
    const slider = await screen.findByRole("slider", {
      name: "Context length",
    });
    act(() => slider.focus());
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    await screen.findByRole("alert");
    await waitFor(() =>
      expect(
        screen
          .getByRole("slider", { name: "Context length" })
          .getAttribute("aria-valuetext"),
      ).toBe("4k"),
    );
  });

  it("shows nothing and sets no timer once the page is left", async () => {
    const { simulator, backend, app } = setup();
    const held = holdUpdates(simulator, backend);
    await userEvent.click(await screen.findByRole("radio", { name: "Dark" }));
    await waitFor(() => expect(held).toHaveLength(1));
    await act(() =>
      app.router.navigate({ to: "/c/$chatId", params: { chatId: "new" } }),
    );
    await waitFor(() => expect(screen.queryByTestId("settings")).toBeNull());
    const setTimeoutSpy = vi.spyOn(window, "setTimeout");
    try {
      act(() => held[0].release());
      await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
      expect(setTimeoutSpy.mock.calls.some(([, delay]) => delay === 3000)).toBe(
        false,
      );
    } finally {
      setTimeoutSpy.mockRestore();
    }
  });

  it("announces each save again", async () => {
    setup();
    const status = await screen.findByRole("status");
    expect(status.textContent).toBe("");
    await userEvent.click(screen.getByRole("radio", { name: "Dark" }));
    await waitFor(() => expect(status.textContent).toBe("Saved"));
    const first = status.firstElementChild;
    await userEvent.click(screen.getByRole("radio", { name: "Light" }));
    await waitFor(() => expect(status.firstElementChild).not.toBe(first));
    expect(status.textContent).toBe("Saved");
    expect(screen.getByRole("status")).toBe(status);
  });

  it("shows the address as saved, keeping the focus", async () => {
    const { simulator, backend } = setup();
    simulator.handle("update_settings", (args) => {
      const patch = args.patch as SettingsPatch;
      // Normalisation du moteur : schéma et port ajoutés.
      backend.settings.serverUrl = `http://${patch.serverUrl}:11434`;
      backend.settings.effectiveServerUrl = backend.settings.serverUrl;
      return { ...backend.settings };
    });
    const address =
      await screen.findByLabelText<HTMLInputElement>("Server address");
    await userEvent.type(address, "192.168.1.20{Enter}");
    await screen.findByText("Saved");
    await waitFor(() =>
      expect(address.value).toBe("http://192.168.1.20:11434"),
    );
    expect(document.activeElement).toBe(address);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled,
    ).toBe(true);
  });
});

describe("Settings tabs", () => {
  const tab = (name: string) => screen.getByTestId(`settings-tab-${name}`);
  const currentTabs = () =>
    within(screen.getByRole("navigation", { name: "Settings" }))
      .getAllByRole("link")
      .filter((link) => link.getAttribute("aria-current") === "page")
      .map((link) => link.textContent);

  it("General by default: the server settings and Reset, links to each tab", async () => {
    setup();
    await screen.findByText("Server address");
    const nav = screen.getByRole("navigation", { name: "Settings" });
    expect(
      within(nav)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    ).toEqual([
      ["General", "/settings"],
      ["Models", "/settings?section=models"],
      ["Account", "/settings?section=account"],
      ["Dictation", "/settings?section=dictation"],
      ["Read aloud", "/settings?section=readAloud"],
      ["Voice mode", "/settings?section=voiceMode"],
      ["Easy Link", "/settings?section=easyLink"],
      ["Keyboard shortcuts", "/settings?section=shortcuts"],
    ]);
    expect(currentTabs()).toEqual(["General"]);
    expect(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(screen.queryByTestId("ollama-account")).toBeNull();
    expect(screen.queryByTestId("dictation")).toBeNull();
    expect(screen.queryByTestId("models-settings")).toBeNull();
  });

  const shown = {
    General: () => screen.findByText("Server address"),
    Models: () => screen.findByTestId("models-settings"),
    Account: () => screen.findByTestId("ollama-account"),
    Dictation: () => screen.findByTestId("dictation"),
    "Easy Link": () => screen.findByTestId("easy-link-settings"),
  };

  it.each([
    ["/settings?section=general", "General"],
    ["/settings?section=models", "Models"],
    ["/settings?section=account", "Account"],
    ["/settings?section=dictation", "Dictation"],
    ["/settings?section=easyLink", "Easy Link"],
    ["/settings?section=other", "General"],
  ] as const)("%s opens %s", async (path, name) => {
    setup({}, path);
    await shown[name]();
    expect(currentTabs()).toEqual([name]);
  });

  it("types the section of each tab, and nothing else", () => {
    for (const section of [
      "general",
      "models",
      "account",
      "dictation",
      "readAloud",
      "voiceMode",
      "easyLink",
      "shortcuts",
    ]) {
      expect(validateSettingsSearch({ section })).toEqual({ section });
    }
    expect(validateSettingsSearch({ section: "other" })).toEqual({});
    expect(validateSettingsSearch({})).toEqual({});
  });

  it("a click shows the tab and replaces the address, without stacking history", async () => {
    const { app } = setup();
    await screen.findByText("Server address");
    const length = app.router.history.length;
    await userEvent.click(tab("dictation"));
    await screen.findByTestId("dictation");
    expect(app.router.state.location.search).toEqual({ section: "dictation" });
    expect(currentTabs()).toEqual(["Dictation"]);
    expect(screen.queryByText("Server address")).toBeNull();
    await userEvent.click(tab("account"));
    await screen.findByTestId("ollama-account");
    await userEvent.click(tab("general"));
    await screen.findByText("Server address");
    expect(app.router.state.location.search).toEqual({});
    expect(app.router.history.length).toBe(length);
    // Changement d'onglet : le focus reste sur l'onglet.
    expect(document.activeElement).toBe(tab("general"));
  });

  it("keyboard: Tab reaches the tabs, Enter opens one", async () => {
    const { app } = setup();
    await screen.findByText("Server address");
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    const user = userEvent.setup();
    // Après le bouton de la barre latérale : les onglets.
    for (let i = 0; i < 5 && document.activeElement !== tab("general"); i++) {
      await user.tab();
    }
    expect(document.activeElement).toBe(tab("general"));
    await user.tab();
    expect(document.activeElement).toBe(tab("models"));
    await user.tab();
    expect(document.activeElement).toBe(tab("account"));
    await user.keyboard("{Enter}");
    await screen.findByTestId("ollama-account");
    expect(app.router.state.location.search).toEqual({ section: "account" });
  });

  it("Ctrl+, from another tab comes back to General", async () => {
    const { app } = setup({}, "/settings?section=dictation");
    await screen.findByTestId("dictation");
    fireEvent.keyDown(document.body, { key: ",", ctrlKey: true });
    await screen.findByText("Server address");
    expect(app.router.state.location.search).toEqual({});
  });

  it("Dictation: one card for each part", async () => {
    setup({}, "/settings?section=dictation");
    const dictation = await screen.findByTestId("dictation");
    await within(dictation).findByLabelText("Instructions");
    const cards = [...dictation.querySelectorAll(":scope > section")];
    const named = (name: string) =>
      cards.find(
        (card) =>
          card.getAttribute("aria-label") === name ||
          card.querySelector("h2")?.textContent === name,
      ) as HTMLElement;
    expect(
      cards.map(
        (card) =>
          card.querySelector("h2")?.textContent ??
          card.getAttribute("aria-label"),
      ),
    ).toEqual([
      "Dictation",
      "Transcription",
      "Polish",
      "Notifications",
      "Try dictation",
    ]);
    const controls = (name: string) =>
      [
        ...named(name).querySelectorAll(
          "select, textarea, input:not([type=radio]), [role=switch], [role=radiogroup]",
        ),
      ].map(
        (control) =>
          control.getAttribute("aria-label") ??
          (control.id
            ? document.querySelector(`label[for="${control.id}"]`)?.textContent
            : null) ??
          (control.getAttribute("aria-labelledby") &&
            document.getElementById(control.getAttribute("aria-labelledby")!)
              ?.textContent),
      );
    expect(controls("Dictation")).toEqual(["Enable dictation", "Mode"]);
    expect(controls("Transcription")).toEqual([
      "Speech model",
      "Language",
      "Dictionary",
      "Copy to clipboard automatically",
      "Paste where the cursor is",
    ]);
    expect(controls("Polish")).toEqual(["Polish model", "Instructions"]);
    expect(controls("Notifications")).toEqual([
      "Show the text in notifications",
    ]);
    expect(
      within(named("Try dictation")).getByRole("button", {
        name: "Try dictation",
      }),
    ).toBeTruthy();
  });
});

describe("Settings in other languages", () => {
  /**
   * Langue réglée et affichée (chaque lecture des réglages rend celle du
   * moteur).
   */
  function speak(backend: ReturnType<typeof fakeBackend>, lang: "fr" | "es") {
    Object.assign(backend.settings, {
      language: lang,
      effectiveLanguage: lang,
    });
    act(() => setLanguage(lang));
  }

  it("General: every text comes from the catalogs, errors included", async () => {
    const { simulator, backend } = setup({ theme: "dark" });
    await screen.findByText("Server address");
    // Reset en échec, puis une ligne en échec : leurs textes aussi.
    backend.settingsErrors.update = { kind: "database", message: "disk full" };
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText(RESET_ERROR);
    consoleError.mockRestore();
    await userEvent.click(screen.getByRole("radio", { name: "Light" }));
    await screen.findByText("Couldn't save this setting: disk full");
    backend.settingsErrors.update = null;
    await userEvent.click(screen.getByRole("radio", { name: "Light" }));
    await waitFor(() => expect(patches(simulator)).toHaveLength(3));

    usePseudoLanguage();
    await screen.findByText("⟦Saved⟧");
    expect(
      untranslatedTexts(screen.getByTestId("settings"), [
        SERVER_URL,
        "disk full",
      ]),
    ).toEqual([]);
    // Titre de la page, au-dessus des onglets.
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "⟦Settings⟧",
    );
  });

  it("says in French and in Spanish that the settings can't be loaded", async () => {
    setup({}, "/settings", (backend) => {
      backend.settingsErrors.get = { kind: "database", message: "locked" };
    });
    await screen.findByText("Failed to load settings");
    act(() => setLanguage("fr"));
    expect(
      screen.getByText("Impossible de charger les paramètres"),
    ).toBeTruthy();
    act(() => setLanguage("es"));
    expect(screen.getByText("No se pudo cargar la configuración")).toBeTruthy();
  });

  it("General in French, then in Spanish, without reloading", async () => {
    const { backend } = setup({ contextLength: 0 });
    await screen.findByText("Server address");
    speak(backend, "fr");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Paramètres",
    );
    const nav = screen.getByRole("navigation", { name: "Paramètres" });
    expect(
      within(nav)
        .getAllByRole("link")
        .map((link) => link.textContent),
    ).toEqual([
      "Général",
      "Modèles",
      "Compte",
      "Dictée",
      "Lecture à voix haute",
      "Mode vocal",
      "Easy Link",
      "Raccourcis clavier",
    ]);
    expect(screen.getByLabelText("Adresse du serveur")).toBeTruthy();
    expect(
      screen
        .getByRole("slider", { name: "Longueur du contexte" })
        .getAttribute("aria-valuetext"),
    ).toBe("Par défaut");
    const themes = within(screen.getByRole("radiogroup", { name: "Thème" }));
    expect(
      themes
        .getAllByRole<HTMLInputElement>("radio")
        .map((radio) => radio.labels?.[0].textContent),
    ).toEqual(["Système", "Clair", "Sombre"]);
    expect(
      screen.getByRole("button", { name: "Rétablir les valeurs par défaut" }),
    ).toBeTruthy();

    speak(backend, "es");
    expect(
      screen.getByRole("navigation", { name: "Configuración" }),
    ).toBeTruthy();
    expect(screen.getByLabelText("Ubicación de los modelos")).toBeTruthy();
    expect(
      screen
        .getByRole("slider", { name: "Longitud del contexto" })
        .getAttribute("aria-valuetext"),
    ).toBe("Por defecto");
    await userEvent.click(screen.getByRole("radio", { name: "Oscuro" }));
    expect(await screen.findByText("Guardado")).toBeTruthy();
  });
});
