// Ligne « Close button » de Settings › General face au simulateur : activée
// par défaut, choix enregistré (« Saved »), grisée avec la raison sans zone
// de notification, erreur montrée, « Reset to defaults » sans effet sur elle,
// textes des catalogues.

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
  "Closing the window keeps ChatonLM running in the system tray, so the shortcuts and the Easy Link bots stay available. Turn off to quit ChatonLM instead.";
const NO_TRAY =
  "There is no system tray on this desktop: closing the window quits ChatonLM.";

function setup({
  trayAvailable = true,
  closeToTray = true,
}: { trayAvailable?: boolean; closeToTray?: boolean } = {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.closeButton = { trayAvailable, closeToTray };
  renderApp("/settings");
  return { simulator, backend };
}

const toggle = () =>
  screen.getByRole("switch", { name: "Keep running in the system tray" });

describe("Close button row", () => {
  it("hides to the tray by default, then quits once turned off", async () => {
    const { simulator, backend } = setup();
    const row = await screen.findByTestId("close-button");
    expect(within(row).getByText("Close button")).toBeTruthy();
    expect(within(row).getByText(DESCRIPTION)).toBeTruthy();
    // Dans la carte General, sous le thème.
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
    expect(simulator.callsTo("set_close_to_tray")).toEqual([
      { enabled: false },
    ]);
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    expect(backend.closeButton.closeToTray).toBe(false);

    await userEvent.click(toggle());
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("true"),
    );
    expect(simulator.callsTo("set_close_to_tray")).toEqual([
      { enabled: false },
      { enabled: true },
    ]);
  });

  it("without a system tray: turned off, disabled, and says why", async () => {
    const { simulator } = setup({ trayAvailable: false });
    const row = await screen.findByTestId("close-button");
    expect(within(row).getByText(NO_TRAY)).toBeTruthy();
    expect(within(row).queryByText(DESCRIPTION)).toBeNull();
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    expect((toggle() as HTMLButtonElement).disabled).toBe(true);
    await userEvent.click(toggle());
    expect(simulator.callsTo("set_close_to_tray")).toEqual([]);
  });

  it("shows a refused save, without Saved", async () => {
    const { simulator } = setup();
    await screen.findByTestId("close-button");
    simulator.handle("set_close_to_tray", () => {
      throw { kind: "database", message: "database is locked" };
    });
    await userEvent.click(toggle());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "database is locked",
    );
    expect(screen.queryByText("Saved")).toBeNull();
    // Relu : rien n'a changé.
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("true"),
    );
  });

  it("is left alone by Reset to defaults", async () => {
    const { simulator, backend } = setup({ closeToTray: false });
    await screen.findByTestId("close-button");
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    expect(simulator.callsTo("set_close_to_tray")).toEqual([]);
    expect(backend.closeButton.closeToTray).toBe(false);
    expect(toggle().getAttribute("aria-checked")).toBe("false");
  });

  it("takes every text from the catalogs", async () => {
    setup();
    await screen.findByTestId("close-button");
    usePseudoLanguage();
    await screen.findByText("⟦Close button⟧");
    expect(untranslatedTexts(screen.getByTestId("close-button"))).toEqual([]);
  });

  it("speaks French and Spanish", async () => {
    setup();
    await screen.findByTestId("close-button");
    act(() => setLanguage("fr"));
    await screen.findByText("Bouton de fermeture");
    expect(
      screen.getByRole("switch", {
        name: "Rester dans la zone de notification",
      }),
    ).toBeTruthy();
    act(() => setLanguage("es"));
    await screen.findByText("Botón de cerrar");
    expect(
      screen.getByRole("switch", { name: "Seguir en la bandeja del sistema" }),
    ).toBeTruthy();
  });
});
