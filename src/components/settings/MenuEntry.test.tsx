// Ligne « Applications menu » de Settings › General face au simulateur :
// seulement sous AppImage, choix enregistré (« Saved »), erreur du moteur
// montrée telle quelle, « Reset to defaults » sans effet sur elle, textes
// des catalogues.

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

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const DESCRIPTION =
  "Show ChatonLM in the applications menu of your desktop, to launch this AppImage. Turn off to remove it.";

function setup({
  available = true,
  enabled = true,
}: { available?: boolean; enabled?: boolean } = {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.menuEntry = { available, enabled };
  renderApp("/settings");
  return { simulator, backend };
}

const toggle = () =>
  screen.getByRole("switch", { name: "Show in the applications menu" });

describe("Applications menu row", () => {
  it("is not shown outside an AppImage", async () => {
    const { simulator } = setup({ available: false });
    await screen.findByText("Theme");
    await waitFor(() =>
      expect(simulator.callsTo("menu_entry_status")).toHaveLength(1),
    );
    expect(screen.queryByText("Applications menu")).toBeNull();
    expect(screen.queryByTestId("menu-entry")).toBeNull();
  });

  it("removes the entry, then shows it again, with Saved each time", async () => {
    const { simulator, backend } = setup();
    const row = await screen.findByTestId("menu-entry");
    expect(within(row).getByText("Applications menu")).toBeTruthy();
    expect(within(row).getByText(DESCRIPTION)).toBeTruthy();
    // Dernière ligne de la carte General, sous le thème.
    const theme = screen.getByRole("radiogroup", { name: "Theme" });
    expect(
      theme.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    expect(toggle().getAttribute("aria-describedby")).toBe(
      within(row).getByText(DESCRIPTION).id,
    );

    await userEvent.click(toggle());
    await screen.findByText("Saved");
    expect(simulator.callsTo("set_menu_entry")).toEqual([{ enabled: false }]);
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    expect(backend.menuEntry.enabled).toBe(false);

    await userEvent.click(toggle());
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("true"),
    );
    expect(simulator.callsTo("set_menu_entry")).toEqual([
      { enabled: false },
      { enabled: true },
    ]);
  });

  it("shows why the file could not change, without Saved", async () => {
    const { backend } = setup();
    await screen.findByTestId("menu-entry");
    backend.menuEntryError = {
      kind: "system",
      message:
        "Could not remove ChatonLM from the applications menu: Permission denied",
    };
    await userEvent.click(toggle());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "Could not remove ChatonLM from the applications menu: Permission denied",
    );
    expect(screen.queryByText("Saved")).toBeNull();
    // Le choix est enregistré quand même : la ligne le relit.
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("false"),
    );
    // Réussi ensuite : l'erreur disparaît.
    backend.menuEntryError = null;
    await userEvent.click(toggle());
    await screen.findByText("Saved");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("is left alone by Reset to defaults", async () => {
    const { simulator, backend } = setup({ enabled: false });
    await screen.findByTestId("menu-entry");
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    expect(simulator.callsTo("set_menu_entry")).toEqual([]);
    expect(backend.menuEntry.enabled).toBe(false);
    expect(toggle().getAttribute("aria-checked")).toBe("false");
  });

  it("takes every text from the catalogs", async () => {
    const { backend } = setup();
    await screen.findByTestId("menu-entry");
    backend.menuEntryError = { kind: "system", message: "denied" };
    await userEvent.click(toggle());
    await screen.findByText("denied");
    usePseudoLanguage();
    await screen.findByText("⟦Applications menu⟧");
    expect(
      untranslatedTexts(screen.getByTestId("menu-entry"), ["denied"]),
    ).toEqual([]);
  });

  it("speaks French and Spanish", async () => {
    setup();
    await screen.findByTestId("menu-entry");
    act(() => setLanguage("fr"));
    await screen.findByText("Menu des applications");
    expect(
      screen.getByRole("switch", {
        name: "Afficher dans le menu des applications",
      }),
    ).toBeTruthy();
    act(() => setLanguage("es"));
    await screen.findByText("Menú de aplicaciones");
    expect(
      screen.getByRole("switch", {
        name: "Mostrar en el menú de aplicaciones",
      }),
    ).toBeTruthy();
  });
});
