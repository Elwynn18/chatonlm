// Ligne « Engine » de Settings › General face au faux moteur : version de
// llama.cpp, accélération (carte nommée, carte sans nom, processeur), repli
// de la session, moteur absent, « Open logs » et son échec, langues.

import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import type { EngineStatus } from "@/bindings/EngineStatus";
import { setLanguage } from "@/i18n";
import { createQueryClient } from "@/lib/queryClient";
import { ENGINE_GPU, fakeEngine } from "@/test/engine";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { installSimulator } from "@/test/simulator";
import { EngineRow } from "./Engine";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

/** La ligne seule ; `status` retouche l'état du faux moteur. */
function renderRow(status: Partial<EngineStatus> = {}) {
  const simulator = installSimulator();
  const engine = fakeEngine(simulator);
  Object.assign(engine.status, status);
  const queryClient = createQueryClient();
  const view = render(
    <QueryClientProvider client={queryClient}>
      <EngineRow descriptionId="engine-description" />
    </QueryClientProvider>,
  );
  onTestFinished(() => {
    cleanup();
    queryClient.clear();
  });
  return { simulator, engine, ...view };
}

const description = () => document.getElementById("engine-description");

describe("EngineRow", () => {
  it("shows the llama.cpp version and the graphics card", async () => {
    renderRow();
    await waitFor(() =>
      expect(description()?.textContent).toBe("llama.cpp v0.9.0"),
    );
    expect(screen.getByText("Engine")).toBeTruthy();
    expect(screen.getByTestId("engine-acceleration").textContent).toBe(
      `Acceleration: ${ENGINE_GPU}`,
    );
    expect(screen.queryByTestId("engine-fallback")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says graphics card without a name, and no version number when unknown", async () => {
    renderRow({ gpuName: null, version: null });
    await waitFor(() =>
      expect(screen.getByTestId("engine-acceleration").textContent).toBe(
        "Acceleration: graphics card",
      ),
    );
    expect(description()?.textContent).toBe("llama.cpp");
  });

  it("says processor only without a card", async () => {
    renderRow({ backend: "cpu", gpuName: null });
    await waitFor(() =>
      expect(screen.getByTestId("engine-acceleration").textContent).toBe(
        "Acceleration: processor only",
      ),
    );
    expect(screen.queryByTestId("engine-fallback")).toBeNull();
  });

  it("explains the fallback to the processor after the card failed", async () => {
    renderRow({ backend: "cpu", cpuFallback: true });
    expect((await screen.findByTestId("engine-fallback")).textContent).toBe(
      "The graphics card failed during this session: models run on the processor until ChatonLM restarts.",
    );
    expect(screen.getByTestId("engine-acceleration").textContent).toBe(
      "Acceleration: processor only",
    );
  });

  it("says the engine is missing from the installation", async () => {
    renderRow({ available: false, version: null });
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The engine is missing from this installation. Reinstall ChatonLM.",
    );
    expect(screen.queryByTestId("engine-acceleration")).toBeNull();
    // Les journaux restent ouvrables : ils disent pourquoi.
    expect(screen.getByRole("button", { name: "Open logs" })).toBeTruthy();
  });

  it("says when the engine state can't be read", async () => {
    const simulator = installSimulator();
    const engine = fakeEngine(simulator);
    engine.failures.set("engine_status", {
      kind: "internal",
      message: "no state",
    });
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <EngineRow descriptionId="engine-description" />
      </QueryClientProvider>,
    );
    onTestFinished(() => {
      cleanup();
      queryClient.clear();
    });
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not read the engine's state: no state",
    );
  });

  it("opens the logs folder, and says when it can't", async () => {
    const { simulator, engine } = renderRow();
    const open = await screen.findByRole("button", { name: "Open logs" });
    await userEvent.click(open);
    await waitFor(() =>
      expect(simulator.callsTo("open_engine_logs")).toHaveLength(1),
    );
    expect(screen.queryByRole("alert")).toBeNull();

    engine.failures.set("open_engine_logs", {
      kind: "system",
      message: "no file manager",
    });
    await userEvent.click(open);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not open the logs: no file manager",
    );
    // Réussi ensuite : l'échec disparaît.
    engine.failures.delete("open_engine_logs");
    await userEvent.click(open);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("speaks French", async () => {
    renderRow({ backend: "cpu", cpuFallback: true });
    act(() => setLanguage("fr"));
    expect((await screen.findByTestId("engine-acceleration")).textContent).toBe(
      "Accélération : processeur seul",
    );
    expect(screen.getByText("Moteur")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Ouvrir les journaux" }));
  });

  it("has every text translated", async () => {
    usePseudoLanguage();
    const { container, engine } = renderRow({ cpuFallback: true });
    engine.failures.set("open_engine_logs", {
      kind: "system",
      message: "no file manager",
    });
    await screen.findByTestId("engine-fallback");
    await userEvent.click(screen.getByRole("button"));
    await screen.findByRole("alert");
    expect(
      untranslatedTexts(container, ["v0.9.0", ENGINE_GPU, "no file manager"]),
    ).toEqual([]);
  });
});
