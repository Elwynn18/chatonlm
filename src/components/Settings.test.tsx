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
import { fakeBackend } from "@/test/backend";
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
    setup({ contextLength: 8192, theme: "dark" });
    await screen.findByTestId("settings");
    // Le moteur et le dossier des modèles à la place de l'adresse d'un
    // serveur.
    expect(screen.getByText("Engine")).toBeTruthy();
    expect(screen.queryByText("Server address")).toBeNull();
    expect(screen.getByText("Model location")).toBeTruthy();
    expect(
      await screen.findByDisplayValue(
        "/home/user/.local/share/com.chatonlm.chatonlm/models",
      ),
    ).toBeTruthy();
    expect(screen.getByText("Context length")).toBeTruthy();
    expect(screen.getByText(CONTEXT_DESCRIPTION)).toBeTruthy();
    expect(screen.getByText("Theme")).toBeTruthy();
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

  it("resets the language, context length, theme and reading aloud, not the model", async () => {
    const { simulator, backend } = setup({
      contextLength: 32768,
      theme: "dark",
      selectedModel: "qwen3.5:0.8b",
      language: "en",
      autoRead: true,
    });
    const held = holdUpdates(simulator, backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Reset to defaults" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].patch).toEqual({
      language: "system",
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
    expect(backend.settings.selectedModel).toBe("qwen3.5:0.8b");
    expect(backend.settings.autoRead).toBe(false);
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
});

describe("Settings tabs", () => {
  const tab = (name: string) => screen.getByTestId(`settings-tab-${name}`);
  const currentTabs = () =>
    within(screen.getByRole("navigation", { name: "Settings" }))
      .getAllByRole("link")
      .filter((link) => link.getAttribute("aria-current") === "page")
      .map((link) => link.textContent);

  it("General by default: the engine settings and Reset, links to each tab", async () => {
    setup();
    await screen.findByText("Model location");
    const nav = screen.getByRole("navigation", { name: "Settings" });
    expect(
      within(nav)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    ).toEqual([
      ["General", "/settings"],
      ["Models", "/settings?section=models"],
      ["Dictation", "/settings?section=dictation"],
      ["Read aloud", "/settings?section=readAloud"],
      ["Voice mode", "/settings?section=voiceMode"],
      ["Easy Link", "/settings?section=easyLink"],
      ["Web search", "/settings?section=webSearch"],
      ["VS Code", "/settings?section=vscode"],
      ["Keyboard shortcuts", "/settings?section=shortcuts"],
    ]);
    expect(currentTabs()).toEqual(["General"]);
    expect(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(screen.queryByTestId("dictation")).toBeNull();
    expect(screen.queryByTestId("models-settings")).toBeNull();
  });

  const shown = {
    General: () => screen.findByText("Model location"),
    Models: () => screen.findByTestId("models-settings"),
    Dictation: () => screen.findByTestId("dictation"),
    "Easy Link": () => screen.findByTestId("easy-link-settings"),
    "Web search": () => screen.findByTestId("web-search-settings"),
    "VS Code": () => screen.findByTestId("vscode-settings"),
  };

  it.each([
    ["/settings?section=general", "General"],
    ["/settings?section=models", "Models"],
    // Ancien onglet du compte d'Ollama : General.
    ["/settings?section=account", "General"],
    ["/settings?section=dictation", "Dictation"],
    ["/settings?section=easyLink", "Easy Link"],
    ["/settings?section=webSearch", "Web search"],
    ["/settings?section=vscode", "VS Code"],
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
      "dictation",
      "readAloud",
      "voiceMode",
      "easyLink",
      "webSearch",
      "vscode",
      "shortcuts",
    ]) {
      expect(validateSettingsSearch({ section })).toEqual({ section });
    }
    expect(validateSettingsSearch({ section: "other" })).toEqual({});
    expect(validateSettingsSearch({ section: "account" })).toEqual({});
    expect(validateSettingsSearch({})).toEqual({});
  });

  it("a click shows the tab and replaces the address, without stacking history", async () => {
    const { app } = setup();
    await screen.findByText("Model location");
    const length = app.router.history.length;
    await userEvent.click(tab("dictation"));
    await screen.findByTestId("dictation");
    expect(app.router.state.location.search).toEqual({ section: "dictation" });
    expect(currentTabs()).toEqual(["Dictation"]);
    expect(screen.queryByText("Model location")).toBeNull();
    await userEvent.click(tab("models"));
    await screen.findByTestId("models-settings");
    await userEvent.click(tab("general"));
    await screen.findByText("Model location");
    expect(app.router.state.location.search).toEqual({});
    expect(app.router.history.length).toBe(length);
    // Changement d'onglet : le focus reste sur l'onglet.
    expect(document.activeElement).toBe(tab("general"));
  });

  it("keyboard: Tab reaches the tabs, Enter opens one", async () => {
    const { app } = setup();
    await screen.findByText("Model location");
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
    expect(document.activeElement).toBe(tab("dictation"));
    await user.keyboard("{Enter}");
    await screen.findByTestId("dictation");
    expect(app.router.state.location.search).toEqual({
      section: "dictation",
    });
  });

  it("Ctrl+, from another tab comes back to General", async () => {
    const { app } = setup({}, "/settings?section=dictation");
    await screen.findByTestId("dictation");
    fireEvent.keyDown(document.body, { key: ",", ctrlKey: true });
    await screen.findByText("Model location");
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
    await screen.findByText("Model location");
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
        "/home/user/.local/share/com.chatonlm.chatonlm/models",
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
    await screen.findByText("Model location");
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
      "Dictée",
      "Lecture à voix haute",
      "Mode vocal",
      "Easy Link",
      "Recherche web",
      "VS Code",
      "Raccourcis clavier",
    ]);
    expect(screen.getByLabelText("Emplacement des modèles")).toBeTruthy();
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
