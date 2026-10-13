// L'app entière (base vérifiée, réglages, ServerGate, routeur de l'app) face au
// simulateur : écran d'erreur des réglages, premier démarrage.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { settingsKey } from "@/hooks/useSettings";
import { fakeBackend } from "@/test/backend";
import { composer } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import { App } from "./App";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const BROKEN = { kind: "database" as const, message: "disk I/O error" };
const SETTINGS_ERROR = /ChatonLM could not read its settings/;

afterEach(() => {
  vi.unstubAllGlobals();
  document.documentElement.className = "";
  document.documentElement.style.colorScheme = "";
});

/**
 * App entière ; `broken` : get_settings échoue dès le départ ; `firstRun` :
 * installation neuve (premier démarrage pas encore fait).
 */
function renderWholeApp({
  broken = false,
  firstRun = false,
}: { broken?: boolean; firstRun?: boolean } = {}) {
  const simulator = installSimulator({
    handlers: { database_problem: () => null },
  });
  const backend = fakeBackend(simulator);
  if (broken) backend.settingsErrors.get = BROKEN;
  if (firstRun) backend.settings.onboardingDone = false;
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  // jsdom n'a pas matchMedia, que useTheme lit (système clair).
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
  // jsdom n'a pas window.scrollTo, que le routeur appelle à chaque navigation.
  const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  render(
    <QueryClientProvider client={client}>
      <App />
    </QueryClientProvider>,
  );
  // Avant la fin du simulateur (ordre inverse d'inscription).
  onTestFinished(() => {
    cleanup();
    client.clear();
    scrollTo.mockRestore();
  });
  return { backend, client };
}

describe("App and its settings", () => {
  it("shows the settings error when they could never be read", async () => {
    renderWholeApp({ broken: true });

    expect(await screen.findByText(SETTINGS_ERROR)).toBeTruthy();
    expect(screen.getByText(/disk I\/O error/)).toBeTruthy();
  });

  it("stays on screen when a later read fails", async () => {
    const { backend, client } = renderWholeApp();
    await waitFor(() => expect(composer()).toBeTruthy());

    backend.settingsErrors.get = BROKEN;
    await act(() => client.refetchQueries({ queryKey: settingsKey }));
    expect(client.getQueryState(settingsKey)?.error).toEqual(BROKEN);
    // Les abonnés de react-query sont prévenus après un setTimeout(0).
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));

    expect(screen.queryByText(SETTINGS_ERROR)).toBeNull();
    expect(composer()).toBeTruthy();
  });
});

describe("App on its first run", () => {
  it("shows the language choice and the introduction before the app", async () => {
    const { backend } = renderWholeApp({ firstRun: true });

    expect(await screen.findByText("Choose your language")).toBeTruthy();
    // Rien de l'app derrière : pas de zone de saisie.
    expect(document.querySelector("textarea")).toBeNull();

    await userEvent.click(screen.getByRole("radio", { name: "Español" }));
    await userEvent.click(
      await screen.findByRole("button", { name: "Continuar" }),
    );
    expect(await screen.findByText("Paso 1 de 4")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Omitir" }));

    // L'app s'affiche, dans la langue choisie, qui est devenue le réglage.
    await waitFor(() =>
      expect(document.querySelector("textarea")).toBeTruthy(),
    );
    expect(screen.queryByText("Paso 1 de 4")).toBeNull();
    expect(document.documentElement.lang).toBe("es");
    expect(backend.settings).toMatchObject({
      language: "es",
      onboardingDone: true,
    });
  });

  it("goes straight to the app once the first run is done", async () => {
    renderWholeApp();
    await waitFor(() => expect(composer()).toBeTruthy());
    expect(screen.queryByText("Choose your language")).toBeNull();
  });
});
