// Visite guidée du premier démarrage dans l'app entière (vraies routes,
// simulateur) : pages ouvertes par les étapes, zone mise en lumière, bulle,
// clavier, fin et abandon.

import {
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setSidebarOpen } from "@/lib/sidebar";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  vi.restoreAllMocks();
  // Fermée au lancement, comme dans l'app.
  setSidebarOpen(false);
});

/** App d'une installation neuve (`done` : visite déjà faite). */
function renderFirstRun({ done = false }: { done?: boolean } = {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.settings.onboardingDone = done;
  const app = renderApp("/c/new");
  const patches = () =>
    simulator.callsTo("update_settings").map((args) => args.patch);
  return { app, backend, patches };
}

const bubble = () => screen.findByRole("dialog");
const title = () =>
  within(screen.getByRole("dialog")).getByRole("heading").textContent;
const button = (name: string) =>
  within(screen.getByRole("dialog")).getByRole("button", { name });
const names = () =>
  within(screen.getByRole("dialog"))
    .getAllByRole("button")
    .map((item) => item.textContent);
const composerArea = () => document.querySelector('[data-tour="composer"]');

const STEPS = [
  { title: "Start a conversation here", path: "/c/new" },
  { title: "Get models in the ModelHub", path: "/hub" },
  { title: "Proofread and Study", path: "/proofread" },
  { title: "Make it yours in Settings", path: "/settings" },
];

describe("Tour", () => {
  it("starts on the chat page, over an app that no longer answers", async () => {
    const { app } = renderFirstRun();

    const dialog = await bubble();
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(title()).toBe(STEPS[0].title);
    expect(within(dialog).getByText("Step 1 of 4")).toBeTruthy();
    expect(dialog.getAttribute("aria-labelledby")).toBe(
      within(dialog).getByRole("heading").id,
    );
    // Première étape : pas de retour.
    expect(names()).toEqual(["Skip", "Next"]);
    expect(app.path()).toBe("/c/new");
    // Barre latérale ouverte : les entrées montrées ensuite sont là.
    await waitFor(() =>
      expect(document.querySelector('nav a[href="/hub"]')).toBeTruthy(),
    );
    // L'app dessous est inerte, la bulle ne l'est pas.
    await waitFor(() => expect(composerArea()).toBeTruthy());
    expect(composerArea()?.closest("[inert]")).toBeTruthy();
    expect(dialog.closest("[inert]")).toBeNull();
    // Entrée avance : le bouton principal a le focus.
    expect(document.activeElement).toBe(button("Next"));
  });

  it("opens the page of each step, back and forth, then ends on a new chat", async () => {
    const { app, backend, patches } = renderFirstRun();
    await bubble();

    for (const [index, step] of STEPS.entries()) {
      await waitFor(() => expect(title()).toBe(step.title));
      expect(screen.getByText(`Step ${index + 1} of 4`)).toBeTruthy();
      await waitFor(() => expect(app.path()).toBe(step.path));
      if (index < STEPS.length - 1) await userEvent.click(button("Next"));
    }
    // Dernière étape : ni « Next » ni « Skip ».
    expect(names()).toEqual(["Back", "Get started"]);
    expect(document.activeElement).toBe(button("Get started"));

    await userEvent.click(button("Back"));
    await waitFor(() => expect(title()).toBe(STEPS[2].title));
    await waitFor(() => expect(app.path()).toBe("/proofread"));
    expect(names()).toEqual(["Skip", "Back", "Next"]);
    await userEvent.click(button("Next"));
    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(backend.settings.onboardingDone).toBe(false);
    expect(patches()).toEqual([]);

    await userEvent.click(button("Get started"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(patches()).toEqual([{ onboardingDone: true }]);
    await waitFor(() => expect(app.path()).toBe("/c/new"));
    // L'app répond de nouveau.
    await waitFor(() => expect(composerArea()).toBeTruthy());
    expect(composerArea()?.closest("[inert]")).toBeNull();
  });

  it("is closed for good by Skip", async () => {
    const { app, backend, patches } = renderFirstRun();
    await bubble();
    await userEvent.click(button("Next"));
    await waitFor(() => expect(app.path()).toBe("/hub"));

    await userEvent.click(button("Skip"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(patches()).toEqual([{ onboardingDone: true }]);
    expect(backend.settings.onboardingDone).toBe(true);
    await waitFor(() => expect(app.path()).toBe("/c/new"));
  });

  it("is closed by Escape, and keeps Tab inside the bubble", async () => {
    const { app, patches } = renderFirstRun();
    await bubble();
    const user = userEvent.setup();
    await user.click(button("Next"));
    await waitFor(() => expect(title()).toBe(STEPS[1].title));
    expect(document.activeElement).toBe(button("Next"));

    await user.tab();
    expect(document.activeElement).toBe(button("Skip"));
    await user.tab();
    expect(document.activeElement).toBe(button("Back"));
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(button("Skip"));
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(button("Next"));
    // Maj+Tab de WebKitGTK : touche « Unidentified », code « Tab ».
    const leftTab = { key: "Unidentified", code: "Tab", shiftKey: true };
    expect(fireEvent.keyDown(button("Next"), leftTab)).toBe(false);
    expect(document.activeElement).toBe(button("Back"));
    expect(fireEvent.keyDown(button("Back"), leftTab)).toBe(false);
    expect(fireEvent.keyDown(button("Skip"), leftTab)).toBe(false);
    expect(document.activeElement).toBe(button("Next"));

    // Les raccourcis de l'app se taisent pendant la visite.
    await user.keyboard("{Control>},{/Control}");
    await user.keyboard("{Control>}n{/Control}");
    expect(app.path()).toBe("/hub");
    expect(screen.getByRole("dialog")).toBeTruthy();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(patches()).toEqual([{ onboardingDone: true }]);
  });

  it("lights up the element of the step, the bubble next to it", async () => {
    // jsdom ne met rien en page : rectangle donné à l'entrée ModelHub.
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
      function (this: Element) {
        const shown = this.matches('nav a[href="/hub"]');
        return (
          shown
            ? {
                left: 16,
                top: 150,
                right: 176,
                bottom: 186,
                width: 160,
                height: 36,
              }
            : { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }
        ) as DOMRect;
      },
    );
    renderFirstRun();
    const dialog = await bubble();

    // Zone de saisie sans taille ici : rien à montrer, bulle au centre.
    await waitFor(() => expect(dialog.className).toContain("opacity-100"), {
      timeout: 3000,
    });
    expect(screen.queryByTestId("tour-spot")).toBeNull();
    expect(dialog.style.left).toBe("50%");
    expect(dialog.style.transform).toBe("translate(-50%, -50%)");

    await userEvent.click(button("Next"));
    const spot = await screen.findByTestId("tour-spot");
    // Marge de 6 px autour de l'entrée ; bulle à sa droite, à 12 px.
    await waitFor(() => expect(spot.style.left).toBe("10px"));
    expect(spot.style.top).toBe("144px");
    expect(spot.style.width).toBe("172px");
    expect(spot.style.height).toBe("48px");
    expect(dialog.style.left).toBe("194px");
    expect(dialog.style.top).toBe("144px");
    expect(dialog.style.transform).toBe("");

    // Étape dont rien n'est affiché : de nouveau au centre.
    await userEvent.click(button("Next"));
    await waitFor(() => expect(screen.queryByTestId("tour-spot")).toBeNull(), {
      timeout: 3000,
    });
    expect(dialog.style.left).toBe("50%");
  });

  it("says why when it cannot be closed, and stays", async () => {
    const { backend, patches } = renderFirstRun();
    const dialog = await bubble();
    backend.settingsErrors.update = {
      kind: "database",
      message: "disk I/O error",
    };

    await userEvent.click(button("Skip"));
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toBe(
      "ChatonLM could not save your choice: disk I/O error",
    );
    expect(title()).toBe(STEPS[0].title);

    backend.settingsErrors.update = null;
    await userEvent.click(button("Skip"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(patches()).toEqual([
      { onboardingDone: true },
      { onboardingDone: true },
    ]);
  });

  it("has no hard-coded text in any of its steps", async () => {
    renderFirstRun();
    const dialog = await bubble();
    usePseudoLanguage();
    for (let step = 0; step < STEPS.length; step++) {
      await waitFor(() =>
        expect(dialog.textContent).toContain(String(step + 1)),
      );
      expect(untranslatedTexts(dialog)).toEqual([]);
      if (step < STEPS.length - 1) {
        // Le bouton principal est le dernier.
        await userEvent.click(
          within(dialog).getAllByRole("button").at(-1) as HTMLElement,
        );
      }
    }
  });

  it("is not shown once done, and leaves the sidebar closed", async () => {
    renderFirstRun({ done: true });
    await waitFor(() => expect(composerArea()).toBeTruthy());
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(composerArea()?.closest("[inert]")).toBeNull();
    expect(document.querySelector('nav a[href="/hub"]')).toBeNull();
  });
});
