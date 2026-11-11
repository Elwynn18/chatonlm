// Onglet « Web search » des réglages face au simulateur : confidentialité
// dite, DuckDuckGo par défaut, choix d'un moteur enregistré aussitôt (« Saved »),
// clé envoyée puis jamais relue (champ vidé, « A key is saved. »), essai d'un
// moteur (résultat annoncé), retrait de la clé (DuckDuckGo reprend), adresse
// SearXNG refusée par le moteur (erreur montrée), lien vers la page des clés,
// onglet ouvert par l'adresse, textes des catalogues (pseudo-langue, français).

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
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import { webSearchOf } from "@/test/webSearch";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const PRIVACY =
  "Searches go from this computer straight to the search engine below. ChatonLM sees none of them; the search engine sees what you search for and your IP address.";

function setup() {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  simulator.handle("open_external", () => null);
  renderApp("/settings?section=webSearch");
  return { simulator, backend, fake: webSearchOf(simulator) };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name });

describe("Web search settings", () => {
  it("is a tab of its own that says who sees the searches", async () => {
    setup();
    const page = await screen.findByTestId("web-search-settings");
    expect(within(page).getByText(PRIVACY)).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Web search" })
        .getAttribute("aria-current"),
    ).toBe("page");
    // DuckDuckGo par défaut, aucun champ de clé.
    const group = screen.getByRole("radiogroup", { name: "Search engine" });
    const names = within(group)
      .getAllByRole("radio")
      .map((input) => (input as HTMLInputElement).value);
    expect(names).toEqual(["duckDuckGo", "brave", "mojeek", "searxng"]);
    expect((radio(/^DuckDuckGo/) as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByLabelText("Brave Search API key")).toBeNull();
  });

  it("saves a key without ever reading it back, tests it, then removes it", async () => {
    const { simulator, fake } = setup();
    await screen.findByTestId("web-search-settings");
    await userEvent.click(radio(/^Brave Search/));
    await screen.findByText("Saved");
    expect(simulator.callsTo("set_web_search_engine")).toEqual([
      { engine: "brave", secret: null },
    ]);
    expect(
      screen.getByText("Until a key is saved, DuckDuckGo is used."),
    ).toBeTruthy();

    const field = screen.getByLabelText("Brave Search API key");
    expect((field as HTMLInputElement).type).toBe("password");
    await userEvent.type(field, "  BSA-secret-123 ");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("A key is saved.");
    expect(fake.secrets.brave).toBe("BSA-secret-123");
    // Champ vidé, et la clé n'est nulle part dans la page.
    expect((field as HTMLInputElement).value).toBe("");
    expect(document.body.innerHTML).not.toContain("BSA-secret");
    expect(
      screen.getByText(
        "If Brave Search doesn't answer, DuckDuckGo takes over.",
      ),
    ).toBeTruthy();

    fake.check = { ok: false, message: "Brave Search refused your key." };
    await userEvent.click(screen.getByRole("button", { name: "Test" }));
    const status = await screen.findByText("Brave Search refused your key.");
    expect(status.getAttribute("role")).toBe("status");
    expect(simulator.callsTo("check_web_search_engine")).toEqual([
      { engine: "brave" },
    ]);

    await userEvent.click(screen.getByRole("button", { name: "Remove key" }));
    await waitFor(() =>
      expect((radio(/^DuckDuckGo/) as HTMLInputElement).checked).toBe(true),
    );
    expect(fake.secrets.brave).toBeUndefined();
    expect(simulator.callsTo("forget_web_search_secret")).toEqual([
      { engine: "brave" },
    ]);
  });

  it("shows the engine's refusal of an address and keeps it in the field", async () => {
    const { fake } = setup();
    await screen.findByTestId("web-search-settings");
    await userEvent.click(radio(/^SearXNG/));
    const field = await screen.findByLabelText("SearXNG address");
    fake.failure = {
      kind: "invalid",
      message: "The SearXNG address isn't valid.",
    };
    await userEvent.type(field, "searx.example");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "The SearXNG address isn't valid.",
    );
    expect((field as HTMLInputElement).value).toBe("searx.example");
    expect(fake.secrets.searxng).toBeUndefined();
  });

  it("links to the page of the keys", async () => {
    const { simulator } = setup();
    await screen.findByTestId("web-search-settings");
    await userEvent.click(radio(/^Mojeek/));
    await userEvent.click(
      await screen.findByRole("button", { name: "Get a Mojeek API key" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://www.mojeek.com/services/search/web-search-api/" },
      ]),
    );
  });

  it("takes its texts from the catalogs", async () => {
    usePseudoLanguage();
    const { fake } = setup();
    fake.engine = "brave";
    fake.secrets.brave = "k";
    await screen.findByTestId("web-search-settings");
    await screen.findByRole("button", { name: /Test/ });
    expect(
      untranslatedTexts(screen.getByTestId("web-search-settings"), [
        "DuckDuckGo",
        "Brave Search",
        "Mojeek",
        "SearXNG",
      ]),
    ).toEqual([]);
  });

  it("speaks French and Spanish", async () => {
    setup();
    await screen.findByTestId("web-search-settings");
    act(() => setLanguage("fr"));
    expect(
      await screen.findByRole("radiogroup", { name: "Moteur de recherche" }),
    ).toBeTruthy();
    expect(screen.getByRole("link", { name: "Recherche web" })).toBeTruthy();
    act(() => setLanguage("es"));
    expect(
      await screen.findByRole("radiogroup", { name: "Buscador" }),
    ).toBeTruthy();
    act(() => setLanguage("en"));
  });
});
