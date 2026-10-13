// Premier démarrage, choix de la langue, face au simulateur : langue du
// bureau proposée, choix appliqué à l'écran et enregistré, suite donnée une
// fois la langue enregistrée.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { installSimulator } from "@/test/simulator";
import { LanguageStep } from "./LanguageStep";

function renderStep() {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.settings.onboardingDone = false;
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  const onDone = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <LanguageStep onDone={onDone} />
    </QueryClientProvider>,
  );
  // Avant la fin du simulateur (ordre inverse d'inscription).
  onTestFinished(() => {
    cleanup();
    client.clear();
  });
  const patches = () =>
    simulator.callsTo("update_settings").map((args) => args.patch);
  return { backend, patches, onDone };
}

const radio = (name: string) =>
  screen.getByRole<HTMLInputElement>("radio", { name });
const click = (name: string) =>
  userEvent.click(screen.getByRole("button", { name }));

describe("LanguageStep", () => {
  it("proposes the desktop language among the three", async () => {
    const { onDone } = renderStep();

    expect(
      await screen.findByRole("heading", { name: "Choose your language" }),
    ).toBeTruthy();
    expect(screen.getByRole("radiogroup", { name: "Language" })).toBeTruthy();
    expect(
      screen
        .getAllByRole("radio")
        .map((option) => option.getAttribute("value")),
    ).toEqual(["en", "es", "fr"]);
    expect(radio("English").checked).toBe(true);
    expect(radio("Español").checked).toBe(false);
    expect(radio("Français").checked).toBe(false);
    // Le titre a le focus : il est lu, et Tab part de lui.
    expect(document.activeElement).toBe(screen.getByRole("heading"));
    expect(onDone).not.toHaveBeenCalled();
  });

  it("switches the screen to the chosen language and saves it", async () => {
    const { backend, patches, onDone } = renderStep();
    await screen.findByText("Choose your language");

    await userEvent.click(radio("Français"));
    expect(await screen.findByText("Choisissez votre langue")).toBeTruthy();
    expect(radio("Français").checked).toBe(true);
    expect(document.documentElement.lang).toBe("fr");
    expect(patches()).toEqual([{ language: "fr" }]);
    expect(backend.settings.language).toBe("fr");
    expect(onDone).not.toHaveBeenCalled();

    // Déjà enregistrée : « Continuer » ne l'envoie pas une seconde fois.
    await click("Continuer");
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(patches()).toEqual([{ language: "fr" }]);
  });

  it("saves the proposed language when nothing is changed", async () => {
    const { backend, patches, onDone } = renderStep();
    await screen.findByText("Choose your language");

    await click("Continue");
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(patches()).toEqual([{ language: "en" }]);
    expect(backend.settings.language).toBe("en");
    // La visite reste à faire.
    expect(backend.settings.onboardingDone).toBe(false);
  });

  it("says why when the language cannot be saved, and goes no further", async () => {
    const { backend, patches, onDone } = renderStep();
    await screen.findByText("Choose your language");
    backend.settingsErrors.update = {
      kind: "database",
      message: "disk I/O error",
    };

    await click("Continue");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "ChatonLM could not save your choice: disk I/O error",
    );
    expect(onDone).not.toHaveBeenCalled();

    // Nouvel essai réussi : le message part, la suite est donnée.
    backend.settingsErrors.update = null;
    await click("Continue");
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(patches()).toEqual([{ language: "en" }, { language: "en" }]);
  });

  it("has no hard-coded text", async () => {
    renderStep();
    await screen.findByRole("heading");
    usePseudoLanguage();
    // Noms des langues : écrits dans leur langue, pas traduits.
    expect(
      untranslatedTexts(document.body, ["English", "Español", "Français"]),
    ).toEqual([]);
  });
});
