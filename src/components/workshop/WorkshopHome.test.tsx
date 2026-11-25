// Accueil de l'Atelier face au simulateur : ouvert depuis la carte de la page
// Tools (Tools surlignée), aucun projet, projets du plus récent au plus ancien
// (état, stratégie, phase, rang dans la file, parties faites), tenus à jour
// par « workshop-event », ouverture d'un projet ; réglages repliés (dossier
// par le sélecteur natif, retour au dossier par défaut, recherche web,
// autonomie) ; textes traduits.

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
import { setSidebarOpen } from "@/lib/sidebar";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import { part } from "@/test/workshop";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(
  prepare: (backend: FakeBackend) => void = () => {},
  path = "/workshop",
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  act(() => setSidebarOpen(true));
  const app = renderApp(path);
  return { simulator, backend, workshop: backend.workshop, app };
}

const cards = () =>
  screen.getAllByTestId("workshop-project").map((card) => ({
    id: card.getAttribute("data-project"),
    state: card.getAttribute("data-state"),
    text: card.textContent,
  }));

describe("Workshop home", () => {
  it("opens from its card on the Tools page, says there is no project yet", async () => {
    const user = userEvent.setup();
    const { app } = setup(() => {}, "/tools");
    await user.click(await screen.findByTestId("tool-card-workshop"));
    expect(await screen.findByTestId("workshop-empty")).toBeTruthy();
    expect(app.path()).toBe("/workshop");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Workshop",
    );
    expect(
      document.querySelector('nav a[href="/tools"]')!.className.split(" "),
    ).toContain("bg-accent");
    await user.click(screen.getByRole("link", { name: "New project" }));
    await screen.findByTestId("workshop-form");
    expect(app.path()).toBe("/workshop/new");
  });

  it("lists the projects, newest first, with their state, strategy, phase, queue position and parts", async () => {
    const user = userEvent.setup();
    const { workshop, app } = setup((b) => {
      const older = b.workshop.seed({
        task: "Guide to compost",
        state: "running",
        strategy: "longForm",
        phase: "build",
      });
      older.parts = [
        part(1, { state: "done", words: 500 }),
        part(2, { state: "split" }),
        part(201, { state: "toReview" }),
        part(202, { state: "running" }),
        part(3),
      ];
      b.workshop.seed({ task: "Report on bees" });
    });
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(cards().map((card) => [card.id, card.state])).toEqual([
      ["2", "queued"],
      ["1", "running"],
    ]);
    const [queued, running] = cards();
    expect(queued.text).toContain("Report on bees");
    expect(queued.text).toContain("Queued");
    expect(queued.text).toContain("Strategy to be chosen");
    expect(queued.text).toContain("Position 1 in the queue");
    expect(running.text).toContain("Working");
    expect(running.text).toContain("Long-form writing");
    expect(running.text).toContain("Phase: Writing");
    // Parties : la partie redécoupée ne compte pas, ses sous-parties si.
    const gauge = await within(
      screen.getAllByTestId("workshop-project")[1],
    ).findByRole("progressbar");
    expect(gauge.getAttribute("aria-valuenow")).toBe("2");
    expect(gauge.getAttribute("aria-valuemax")).toBe("4");
    expect(gauge.getAttribute("aria-valuetext")).toBe("2/4 parts");

    // Le moteur prévient : la liste est relue.
    await workshop.update(2, { state: "running", phase: "strategy" });
    await waitFor(() => expect(cards()[0].text).toContain("Phase: Strategy"));
    expect(cards()[0].text).not.toContain("in the queue");

    await user.click(screen.getByRole("link", { name: /Guide to compost/ }));
    await screen.findByTestId("workshop-project-view");
    expect(app.path()).toBe("/workshop/1");
  });

  it("changes the projects folder with the native picker, goes back to the default one, and saves the other settings", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup();
    await screen.findByTestId("workshop-empty");
    const toggle = screen.getByRole("button", { name: "Workshop settings" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const panel = screen.getByTestId("workshop-settings");
    await waitFor(() =>
      expect(screen.getByTestId("workshop-settings-dir").textContent).toBe(
        "/home/user/Documents/ChatonLM",
      ),
    );
    expect(panel.textContent).toContain("Default");
    expect(
      within(panel).queryByRole("button", { name: "Use the default folder" }),
    ).toBeNull();

    // Sélecteur annulé : rien n'est enregistré.
    await user.click(within(panel).getByRole("button", { name: "Choose…" }));
    expect(simulator.callsTo("workshop_set_settings")).toEqual([]);

    workshop.pickedDir = "/data/Projects";
    await user.click(within(panel).getByRole("button", { name: "Choose…" }));
    await waitFor(() =>
      expect(screen.getByTestId("workshop-settings-dir").textContent).toBe(
        "/data/Projects",
      ),
    );
    expect(panel.textContent).not.toContain("Default");
    await user.click(
      within(panel).getByRole("button", { name: "Use the default folder" }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("workshop-settings-dir").textContent).toBe(
        "/home/user/Documents/ChatonLM",
      ),
    );
    // Le bouton a disparu : le focus est passé à « Choose… ».
    expect(document.activeElement).toBe(
      within(panel).getByRole("button", { name: "Choose…" }),
    );

    // Pendant l'enregistrement : la case garde le focus (aria-disabled, pas
    // disabled) et un second clic n'envoie rien.
    const web = within(panel).getByRole<HTMLInputElement>("checkbox", {
      name: /Web search/,
    });
    expect(web.checked).toBe(true);
    const release = workshop.hold("workshop_set_settings");
    await user.click(web);
    await waitFor(() => expect(web.getAttribute("aria-disabled")).toBe("true"));
    expect(web.disabled).toBe(false);
    expect(document.activeElement).toBe(web);
    await user.click(web);
    expect(simulator.callsTo("workshop_set_settings")).toHaveLength(3);
    await act(async () => release());
    await waitFor(() => expect(web.checked).toBe(false));
    expect(web.getAttribute("aria-disabled")).toBeNull();
    expect(document.activeElement).toBe(web);
    await user.selectOptions(
      within(panel).getByRole("combobox", {
        name: "Autonomy of new projects",
      }),
      "autonomous",
    );
    await waitFor(() => expect(workshop.settings.autonomy).toBe("autonomous"));
    expect(simulator.callsTo("workshop_set_settings")).toEqual([
      { patch: { dir: "/data/Projects" } },
      { patch: { dir: "" } },
      { patch: { web: false } },
      { patch: { autonomy: "autonomous" } },
    ]);

    // Refus du moteur : dit, sans rien changer.
    workshop.failures.set("workshop_set_settings", {
      kind: "invalid",
      message: "The folder can't be written to.",
    });
    await user.click(web);
    expect((await within(panel).findByRole("alert")).textContent).toBe(
      "Couldn't save the settings: The folder can't be written to.",
    );
    expect(web.checked).toBe(false);
  });

  it("is translated: French, Spanish, no text outside the catalogs", async () => {
    const user = userEvent.setup();
    setup((b) => {
      b.workshop.seed({ task: "Guide", state: "waiting", strategy: "generic" });
    });
    await waitFor(() => expect(cards()).toHaveLength(1));
    await user.click(screen.getByRole("button", { name: "Workshop settings" }));
    await screen.findByTestId("workshop-settings-dir");
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
        "Atelier",
      ),
    );
    expect(cards()[0].text).toContain("Vous attend");
    act(() => setLanguage("es"));
    await waitFor(() => expect(cards()[0].text).toContain("Te necesita"));
    act(() => setLanguage("en"));

    usePseudoLanguage();
    expect(
      untranslatedTexts(screen.getByTestId("workshop-home"), [
        "Guide",
        "/home/user/Documents/ChatonLM",
      ]),
    ).toEqual([]);
  });
});
