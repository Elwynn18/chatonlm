// Premier démarrage face au simulateur : choix de la langue (appliqué à
// l'écran et enregistré), présentation en quatre étapes, fin et abandon.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished } from "vitest";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { installSimulator } from "@/test/simulator";
import { Onboarding } from "./Onboarding";

function renderOnboarding() {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.settings.onboardingDone = false;
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Onboarding />
    </QueryClientProvider>,
  );
  // Avant la fin du simulateur (ordre inverse d'inscription).
  onTestFinished(() => {
    cleanup();
    client.clear();
  });
  const patches = () =>
    simulator.callsTo("update_settings").map((args) => args.patch);
  return { backend, patches };
}

const title = () => screen.getByRole("heading", { level: 1 }).textContent;
const radio = (name: string) =>
  screen.getByRole<HTMLInputElement>("radio", { name });
const click = (name: string) =>
  userEvent.click(screen.getByRole("button", { name }));

describe("Onboarding", () => {
  it("asks for the language first, with the desktop language proposed", async () => {
    renderOnboarding();

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
  });

  it("switches the screen to the chosen language and saves it", async () => {
    const { backend, patches } = renderOnboarding();
    await screen.findByText("Choose your language");

    await userEvent.click(radio("Français"));
    expect(await screen.findByText("Choisissez votre langue")).toBeTruthy();
    expect(radio("Français").checked).toBe(true);
    expect(document.documentElement.lang).toBe("fr");
    expect(patches()).toEqual([{ language: "fr" }]);
    expect(backend.settings.language).toBe("fr");

    // Déjà enregistrée : « Continuer » ne l'envoie pas une seconde fois.
    await click("Continuer");
    expect(await screen.findByText("Étape 1 sur 4")).toBeTruthy();
    expect(patches()).toEqual([{ language: "fr" }]);
  });

  it("saves the proposed language when nothing is changed", async () => {
    const { backend, patches } = renderOnboarding();
    await screen.findByText("Choose your language");

    await click("Continue");
    expect(await screen.findByText("Step 1 of 4")).toBeTruthy();
    expect(patches()).toEqual([{ language: "en" }]);
    expect(backend.settings.language).toBe("en");
    expect(backend.settings.onboardingDone).toBe(false);
  });

  it("walks through four steps, back and forth, then finishes", async () => {
    const { backend, patches } = renderOnboarding();
    await screen.findByText("Choose your language");
    await click("Continue");

    const titles = [
      "Chat with AI on your own computer",
      "Pick a model in the ModelHub",
      "Proofread and Study",
      "Speak and listen",
    ];
    for (const [index, expected] of titles.entries()) {
      await waitFor(() => expect(title()).toBe(expected));
      expect(screen.getByText(`Step ${index + 1} of 4`)).toBeTruthy();
      // Titre de l'étape lu à son arrivée.
      expect(document.activeElement).toBe(screen.getByRole("heading"));
      const dots = [...screen.getByTestId("onboarding-dots").children];
      expect(dots.map((dot) => dot.className.includes("bg-accent"))).toEqual(
        titles.map((_, dot) => dot === index),
      );
      if (index < titles.length - 1) await click("Next");
    }
    // Dernière étape : plus de « Next » ni de « Skip ».
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull();

    await click("Back");
    await waitFor(() => expect(title()).toBe(titles[2]));
    await click("Back");
    await click("Back");
    await click("Back");
    await waitFor(() => expect(title()).toBe("Choose your language"));
    await click("Continue");
    for (let step = 1; step < titles.length; step++) await click("Next");
    expect(backend.settings.onboardingDone).toBe(false);

    await click("Get started");
    await waitFor(() => expect(backend.settings.onboardingDone).toBe(true));
    expect(patches()).toEqual([{ language: "en" }, { onboardingDone: true }]);
  });

  it("can be skipped from any step but the last", async () => {
    const { backend, patches } = renderOnboarding();
    await screen.findByText("Choose your language");
    // Pas avant d'avoir choisi la langue.
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull();
    await click("Continue");
    await screen.findByText("Step 1 of 4");
    await click("Next");
    await screen.findByText("Step 2 of 4");

    await click("Skip");
    await waitFor(() => expect(backend.settings.onboardingDone).toBe(true));
    expect(patches().at(-1)).toEqual({ onboardingDone: true });
  });

  it("says why when a choice cannot be saved, and stays on the step", async () => {
    const { backend, patches } = renderOnboarding();
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
    expect(title()).toBe("Choose your language");

    // Nouvel essai réussi : le message part, la présentation commence.
    backend.settingsErrors.update = null;
    await click("Continue");
    expect(await screen.findByText("Step 1 of 4")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(patches()).toEqual([{ language: "en" }, { language: "en" }]);
  });

  it("has no hard-coded text on any of its pages", async () => {
    renderOnboarding();
    await screen.findByRole("heading");
    usePseudoLanguage();
    // Noms des langues : écrits dans leur langue, pas traduits.
    const allow = ["English", "Español", "Français"];

    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await userEvent.click(screen.getByRole("button"));
    for (let step = 1; step <= 4; step++) {
      await waitFor(() =>
        expect(screen.getByTestId("onboarding-dots")).toBeTruthy(),
      );
      expect(untranslatedTexts(document.body, allow)).toEqual([]);
      if (step < 4) {
        // « Back », « Next », « Skip » : le deuxième bouton avance.
        await userEvent.click(screen.getAllByRole("button")[1]);
      }
    }
  });
});
