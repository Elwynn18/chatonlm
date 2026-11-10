// Dossier des modèles de Settings › General face au faux moteur : chemin et
// résumé, sélecteur annulé, confirmation (déplacer, utiliser tel quel,
// annuler), sans question quand il n'y a aucun modèle, aucun déchargement
// par la page (le moteur s'en charge s'il accepte), progression, « Saved », copies restées dans l'ancien
// dossier, refus du moteur tel quel, autre échec, langues.

import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import type { EngineModelsDir } from "@/bindings/EngineModelsDir";
import { setLanguage } from "@/i18n";
import { createQueryClient } from "@/lib/queryClient";
import { DEFAULT_MODELS_DIR, fakeEngine } from "@/test/engine";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { installSimulator } from "@/test/simulator";
import { ModelsFolder } from "./ModelsFolder";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const NEW = "/data/Models";
const GIB = 1024 ** 3;

/** Le champ seul ; `dir` retouche le dossier du faux moteur. */
function renderFolder(dir: Partial<EngineModelsDir> = {}) {
  const simulator = installSimulator();
  const engine = fakeEngine(simulator);
  Object.assign(engine.modelsDir, dir);
  // Commande que la page ne doit plus appeler (comptée par les essais).
  simulator.handle("unload_all_models", () => null);
  const onSaved = vi.fn();
  const queryClient = createQueryClient();
  const view = render(
    <QueryClientProvider client={queryClient}>
      <p id="folder-description">Location where models are stored.</p>
      <ModelsFolder
        inputId="folder"
        describedBy="folder-description"
        onSaved={onSaved}
      />
    </QueryClientProvider>,
  );
  onTestFinished(() => {
    cleanup();
    queryClient.clear();
  });
  return { simulator, engine, onSaved, queryClient, ...view };
}

const field = () => document.getElementById("folder") as HTMLInputElement;
/** « Change… » (dans toute langue : le seul bouton hors de la confirmation). */
const changeButton = () =>
  screen.getAllByRole<HTMLButtonElement>("button", { hidden: true })[0];
const dialog = () => screen.queryByRole("alertdialog");
const summary = () => screen.getByTestId("models-folder-summary").textContent;

/** « Change… », avec `path` choisi dans le sélecteur. */
async function change(
  engine: ReturnType<typeof fakeEngine>,
  path: string | null = NEW,
) {
  engine.picks.push(path);
  await waitFor(() => expect(changeButton().disabled).toBe(false));
  await userEvent.click(changeButton());
}

describe("ModelsFolder", () => {
  it("shows the folder, its models, and the free space", async () => {
    renderFolder();
    await waitFor(() => expect(field().value).toBe(DEFAULT_MODELS_DIR));
    expect(changeButton().textContent).toBe("Change…");
    expect(field().readOnly).toBe(true);
    expect(field().getAttribute("aria-describedby")).toBe("folder-description");
    expect(summary()).toBe("Default folder·3 models (9.5 GB)·120 GB free");
  });

  it("drops what it doesn't know, and says when there is no model", async () => {
    renderFolder({
      path: NEW,
      isDefault: false,
      models: 0,
      bytes: 0,
      freeBytes: null,
    });
    await waitFor(() => expect(field().value).toBe(NEW));
    expect(summary()).toBe("No models yet");
  });

  it("does nothing when the folder picker is cancelled, or the same folder is chosen", async () => {
    const { simulator, engine } = renderFolder();
    await change(engine, null);
    await waitFor(() =>
      expect(simulator.callsTo("pick_models_folder")).toHaveLength(1),
    );
    await change(engine, DEFAULT_MODELS_DIR);
    await waitFor(() =>
      expect(simulator.callsTo("pick_models_folder")).toHaveLength(2),
    );
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("unload_all_models")).toHaveLength(0);
    expect(simulator.callsTo("change_engine_models_dir")).toHaveLength(0);
  });

  it("moves the models after confirmation, with progress, then Saved", async () => {
    const { simulator, engine, onSaved } = renderFolder({
      models: 2,
      bytes: 4 * GIB,
    });
    engine.hold = true;
    await change(engine);
    const confirm = await screen.findByRole("alertdialog", {
      name: "Move your models to the new folder?",
    });
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual([
      "ChatonLM will keep your models in /data/Models.",
      `Your 2 models (4.0 GB) are in ${DEFAULT_MODELS_DIR}.`,
      "Use the new folder as it is only if it already has these models.",
      "Loaded models will be unloaded first.",
    ]);
    expect(
      within(confirm)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Cancel", "Use it as it is", "Move models"]);
    expect(document.activeElement).toBe(
      within(confirm).getByRole("button", { name: "Move models" }),
    );

    await userEvent.click(
      within(confirm).getByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(engine.held).toHaveLength(1));
    expect(engine.held[0].args).toEqual({ path: NEW, moveModels: true });
    // La page ne décharge rien : le moteur le fait s'il accepte.
    expect(simulator.callsTo("unload_all_models")).toHaveLength(0);
    expect(dialog()).toBeNull();
    expect(changeButton().disabled).toBe(true);

    const held = engine.held[0];
    await held.channel.send({ kind: "total", bytes: 4 * GIB });
    await held.channel.send({ kind: "progress", done: 1 * GIB });
    expect(screen.getByText("Moving models…")).toBeTruthy();
    const bar = screen.getByRole("progressbar", { name: "Moving models" });
    expect(bar.getAttribute("aria-valuenow")).toBe("25");
    expect(bar.getAttribute("aria-valuetext")).toBe("1.0 GB of 4.0 GB (25%)");
    expect(onSaved).not.toHaveBeenCalled();

    await act(async () => held.finish());
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("progressbar")).toBeNull();
    // Dossier relu.
    await waitFor(() => expect(field().value).toBe(NEW));
    expect(changeButton().disabled).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("uses the new folder as it is without moving anything", async () => {
    const { engine, onSaved } = renderFolder();
    await change(engine);
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Use it as it is" }),
    );
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(engine.modelsDir.path).toBe(NEW);
    await waitFor(() => expect(field().value).toBe(NEW));
    // Rien n'a été déplacé : pas de progression.
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("changes nothing when the confirmation is cancelled", async () => {
    const { simulator, engine, onSaved } = renderFolder();
    await change(engine);
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Cancel" }),
    );
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("unload_all_models")).toHaveLength(0);
    expect(simulator.callsTo("change_engine_models_dir")).toHaveLength(0);
    expect(onSaved).not.toHaveBeenCalled();
    expect(changeButton().disabled).toBe(false);
  });

  it("asks nothing when there is no model to move", async () => {
    const { simulator, engine, onSaved } = renderFolder({
      models: 0,
      bytes: 0,
    });
    await change(engine);
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("change_engine_models_dir")).toEqual([
      expect.objectContaining({ path: NEW, moveModels: true }),
    ]);
  });

  it("says when copies could not be deleted from the old folder", async () => {
    const { engine, onSaved } = renderFolder({ models: 0, bytes: 0 });
    engine.moved = { cleanupFailed: true };
    await change(engine);
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(
      await screen.findByText(
        `Some copies could not be deleted from the old folder ${DEFAULT_MODELS_DIR}. You can delete them yourself.`,
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows the engine's refusal as it is, and unloads nothing", async () => {
    const { simulator, engine, onSaved } = renderFolder({
      models: 0,
      bytes: 0,
    });
    const accepted = vi.fn();
    engine.onAccepted = accepted;
    const refusal =
      "Wait for the model downloads to finish (or cancel them) before moving the models.";
    engine.failures.set("change_engine_models_dir", {
      kind: "invalid",
      message: refusal,
    });
    await change(engine);
    expect((await screen.findByRole("alert")).textContent).toBe(refusal);
    expect(simulator.callsTo("unload_all_models")).toHaveLength(0);
    expect(accepted).not.toHaveBeenCalled();
    expect(onSaved).not.toHaveBeenCalled();
    expect(changeButton().disabled).toBe(false);
    expect(field().value).toBe(DEFAULT_MODELS_DIR);
  });

  it("says why another failure happened", async () => {
    const { simulator, engine, onSaved } = renderFolder({
      models: 0,
      bytes: 0,
    });
    engine.failures.set("change_engine_models_dir", {
      kind: "engine",
      message: "the engine is stuck",
    });
    await change(engine);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not change the models folder: the engine is stuck",
    );
    expect(simulator.callsTo("unload_all_models")).toHaveLength(0);
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("refreshes the installed models once the folder changed", async () => {
    const { engine, queryClient } = renderFolder({ models: 0, bytes: 0 });
    queryClient.setQueryData(["models"], []);
    queryClient.setQueryData(["hub", "installed"], []);
    await change(engine);
    await waitFor(() =>
      expect(queryClient.getQueryState(["models"])?.isInvalidated).toBe(true),
    );
    expect(queryClient.getQueryState(["hub", "installed"])?.isInvalidated).toBe(
      true,
    );
  });

  it("speaks French", async () => {
    const { engine } = renderFolder();
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(summary()).toBe(
        "Dossier par défaut·3 modèles (9,5 Go)·120 Go libres",
      ),
    );
    await change(engine);
    expect(
      await screen.findByRole("alertdialog", {
        name: "Déplacer vos modèles vers le nouveau dossier ?",
      }),
    ).toBeTruthy();
  });

  it("has every text translated", async () => {
    usePseudoLanguage();
    const { engine } = renderFolder();
    engine.hold = true;
    await change(engine);
    await screen.findByRole("alertdialog");
    expect(
      untranslatedTexts(document.body, [
        DEFAULT_MODELS_DIR,
        NEW,
        "Location where models are stored.",
      ]),
    ).toEqual([]);
    await userEvent.click(
      within(screen.getByRole("alertdialog")).getAllByRole("button")[2],
    );
    await waitFor(() => expect(engine.held).toHaveLength(1));
    await engine.held[0].channel.send({ kind: "total", bytes: 4 * GIB });
    await engine.held[0].channel.send({ kind: "progress", done: GIB });
    expect(screen.getByRole("progressbar")).toBeTruthy();
    expect(
      untranslatedTexts(document.body, [
        DEFAULT_MODELS_DIR,
        "Location where models are stored.",
      ]),
    ).toEqual([]);
    await act(async () => engine.held[0].finish({ cleanupFailed: true }));
    await screen.findByText(/⟦/, { selector: "p[aria-live]" });
    expect(
      untranslatedTexts(document.body, [
        DEFAULT_MODELS_DIR,
        NEW,
        "Location where models are stored.",
      ]),
    ).toEqual([]);
  });
});
