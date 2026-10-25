// Carte « Support ChatonLM » de Settings › General face au simulateur : la
// dernière de la page, sous « Reset to defaults », seulement dans General ;
// le bouton ouvre la page des dons du site ; un échec donne l'avis des liens ;
// textes des catalogues.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setLanguage } from "@/i18n";
import { resetLinks } from "@/lib/openLink";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

// Un second clic sur la même adresse en moins d'une seconde est ignoré : d'un
// essai à l'autre, ce n'en est pas un.
afterEach(() => resetLinks());

const DESCRIPTION =
  "Donations keep ChatonLM growing. In particular, they will pay for servers of its own, so that web search no longer depends on Ollama.";

function setup(path = "/settings") {
  const simulator = installSimulator();
  fakeBackend(simulator);
  simulator.handle("open_external", () => null);
  renderApp(path);
  return { simulator };
}

describe("Support ChatonLM card", () => {
  it("is the last card of General, under Reset to defaults", async () => {
    setup();
    const card = await screen.findByRole("region", {
      name: "Support ChatonLM",
    });
    expect(within(card).getByText(DESCRIPTION)).toBeTruthy();
    const reset = screen.getByRole("button", { name: "Reset to defaults" });
    expect(
      reset.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // Dernière carte de la page.
    const sections = screen.getByTestId("settings").querySelectorAll("section");
    expect(sections[sections.length - 1]).toBe(card);
    expect(
      within(card)
        .getByRole("button", { name: "Donate" })
        .getAttribute("aria-describedby"),
    ).toBe(within(card).getByText(DESCRIPTION).id);
  });

  it("is not shown in the other tabs", async () => {
    setup("/settings?section=models");
    await screen.findByTestId("settings-tabs");
    await screen.findByRole("link", { name: "Models", current: "page" });
    expect(screen.queryByTestId("donate")).toBeNull();
  });

  it("opens the donation page of the website", async () => {
    const { simulator } = setup();
    const card = await screen.findByTestId("donate");
    await userEvent.click(within(card).getByRole("button", { name: "Donate" }));
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://chatonlm.com/donate" },
      ]),
    );
  });

  it("a page that can't be opened shows the link notice", async () => {
    const { simulator } = setup();
    simulator.handle("open_external", () => {
      throw { kind: "internal", message: "xdg-open not found" };
    });
    const card = await screen.findByTestId("donate");
    await userEvent.click(within(card).getByRole("button", { name: "Donate" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "https://chatonlm.com/donate",
    );
  });

  it("has no untranslated text", async () => {
    setup();
    await screen.findByTestId("donate");
    usePseudoLanguage();
    await screen.findByText("⟦Support ChatonLM⟧");
    expect(untranslatedTexts(screen.getByTestId("donate"))).toEqual([]);
  });

  it("speaks French and Spanish", async () => {
    setup();
    await screen.findByTestId("donate");
    act(() => setLanguage("fr"));
    expect(
      await screen.findByRole("region", { name: "Soutenir ChatonLM" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Faire un don" })).toBeTruthy();
    act(() => setLanguage("es"));
    expect(
      await screen.findByRole("region", { name: "Apoyar ChatonLM" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Donar" })).toBeTruthy();
  });
});
