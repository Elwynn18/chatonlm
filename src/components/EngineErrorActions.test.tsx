// Actions sous une panne du moteur dans le chat : Retry seulement pour une
// panne (pas pour un moteur absent), journaux du moteur (échec dit), écran
// du moteur ; textes des catalogues, en français.

import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import { setLanguage } from "@/i18n";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { installSimulator } from "@/test/simulator";
import { isEngineError } from "@/lib/engine";
import { EngineErrorActions } from "./EngineErrorActions";

afterEach(() => cleanup());

const names = () =>
  screen.getAllByRole("button").map((button) => button.textContent);

describe("actions under an engine failure", () => {
  it("knows its kinds", () => {
    expect(isEngineError("engineFailed")).toBe(true);
    expect(isEngineError("engineUnavailable")).toBe(true);
    for (const kind of ["connectionLost", "modelMissing", undefined] as (
      ChatErrorKind | undefined
    )[]) {
      expect(isEngineError(kind)).toBe(false);
    }
  });

  it("a failure offers Retry, the logs and the engine's screen", async () => {
    const simulator = installSimulator();
    simulator.handle("open_engine_logs", () => null);
    const onRetry = vi.fn();
    const onOpenSettings = vi.fn();
    render(
      <EngineErrorActions
        kind="engineFailed"
        onRetry={onRetry}
        onOpenSettings={onOpenSettings}
      />,
    );
    expect(names()).toEqual(["Retry", "Open logs", "Engine settings"]);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("button", { name: "Open logs" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Engine settings" }),
    );
    expect(onRetry).toHaveBeenCalledTimes(1);
    expect(simulator.callsTo("open_engine_logs")).toHaveLength(1);
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("a missing engine has nothing to retry", () => {
    installSimulator();
    render(
      <EngineErrorActions
        kind="engineUnavailable"
        onRetry={() => {}}
        onOpenSettings={() => {}}
      />,
    );
    expect(names()).toEqual(["Open logs", "Engine settings"]);
  });

  it("says why the logs could not be opened", async () => {
    const simulator = installSimulator();
    simulator.handle("open_engine_logs", () => {
      throw { kind: "internal", message: "no file manager" };
    });
    render(
      <EngineErrorActions kind="engineFailed" onOpenSettings={() => {}} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Open logs" }));
    expect(
      await screen.findByText("Could not open the logs: no file manager"),
    ).toBeTruthy();
  });

  it("has every text translated", () => {
    installSimulator();
    usePseudoLanguage();
    const { container } = render(
      <EngineErrorActions
        kind="engineFailed"
        onRetry={() => {}}
        onOpenSettings={() => {}}
      />,
    );
    expect(untranslatedTexts(container)).toEqual([]);
  });

  it("speaks French", () => {
    installSimulator();
    act(() => setLanguage("fr"));
    render(
      <EngineErrorActions
        kind="engineFailed"
        onRetry={() => {}}
        onOpenSettings={() => {}}
      />,
    );
    expect(names()).toEqual([
      "Réessayer",
      "Ouvrir les journaux",
      "Paramètres du moteur",
    ]);
  });
});
