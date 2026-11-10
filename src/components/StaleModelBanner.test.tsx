// Bandeau « A newer version of ... is available » face au simulateur : seul
// (états, Update, Dismiss, échec) puis dans l'app (au-dessus de la zone de
// saisie, pour le modèle choisi).

import { QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  configure,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import { useDownloadEvents } from "@/hooks/useDownloads";
import { createQueryClient } from "@/lib/queryClient";
import { type FakeBackend, fakeBackend, modelInfo } from "@/test/backend";
import { renderHub } from "@/test/renderHub";
import { installSimulator } from "@/test/simulator";
import { StaleModelBanner } from "./StaleModelBanner";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const QWEN = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
const QWEN_NAME = "Qwen3-0.6B:Q4_K_M";
const OTHER = "unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M";
const AVAILABLE = `A newer version of ${QWEN_NAME} is available`;
/** Modèles installés de ces essais (faux moteur). */
const INSTALLED = { selectedModel: QWEN, models: [QWEN, OTHER] };

/** Écoute les téléchargements, comme la route racine de l'app. */
function DownloadEvents() {
  useDownloadEvents();
  return null;
}

/**
 * Le bandeau seul, pour `model` (installé ; null : aucun modèle choisi) ;
 * `prepare` règle le faux backend avant le premier rendu.
 */
function renderBanner({
  model = QWEN as string | null,
  isStreaming = false,
  prepare = (backend: FakeBackend) => void backend.outdated.add(QWEN),
} = {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, INSTALLED);
  prepare(backend);
  const queryClient = createQueryClient();
  const show = (shown: string | null, streaming = isStreaming) => (
    <QueryClientProvider client={queryClient}>
      <DownloadEvents />
      <StaleModelBanner
        model={shown === null ? null : modelInfo(shown)}
        isStreaming={streaming}
      />
    </QueryClientProvider>
  );
  const view = render(show(model));
  onTestFinished(() => {
    cleanup();
    queryClient.clear();
  });
  return {
    simulator,
    backend,
    hub: backend.hub,
    /** Autre modèle choisi, ou fin de la réponse. */
    rerender: (shown: string | null, streaming = isStreaming) =>
      view.rerender(show(shown, streaming)),
  };
}

const banner = () => screen.queryByTestId("stale-model");
const update = () => screen.getByRole("button", { name: "Update" });

describe("StaleModelBanner", () => {
  it("says that Hugging Face has a newer version of the selected model", async () => {
    const { simulator } = renderBanner();
    expect(await screen.findByText(AVAILABLE)).toBeTruthy();
    expect(banner()?.getAttribute("role")).toBe("status");
    expect(update()).toHaveProperty("disabled", false);
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeTruthy();
    // Seul le modèle choisi est demandé.
    expect(simulator.callsTo("outdated_models")).toEqual([{ models: [QWEN] }]);
  });

  it("shows nothing for an up-to-date model, nor without a model", async () => {
    const { simulator, rerender } = renderBanner({
      prepare: (backend) => void backend.outdated.add(OTHER),
    });
    await waitFor(() =>
      expect(simulator.callsTo("outdated_models")).toHaveLength(1),
    );
    expect(banner()).toBeNull();
    rerender(null);
    expect(banner()).toBeNull();
    // Rien n'est demandé sans modèle.
    expect(simulator.callsTo("outdated_models")).toHaveLength(1);
  });

  it("shows nothing when Hugging Face can't tell", async () => {
    const { simulator } = renderBanner({
      prepare: (backend) => {
        backend.outdated.add(QWEN);
        backend.outdatedError = { kind: "internal", message: "offline" };
      },
    });
    await waitFor(() =>
      expect(simulator.callsTo("outdated_models")).toHaveLength(1),
    );
    expect(banner()).toBeNull();
  });

  it("names the model by its readable name", async () => {
    const model = "unsloth/Qwen3.5-9B-GGUF:Q4_K_M";
    renderBanner({
      model,
      prepare: (backend) => void backend.outdated.add(model),
    });
    expect(
      await screen.findByText(
        "A newer version of Qwen3.5-9B:Q4_K_M is available",
      ),
    ).toBeTruthy();
  });

  it("is never shown for a model taken from Ollama nor a remote model", async () => {
    for (const model of [
      "@ollama/qwen3:0.6b",
      "@remote/mistral/mistral-small-latest",
    ]) {
      const { simulator } = renderBanner({
        model,
        prepare: (backend) => void backend.outdated.add(model),
      });
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(banner()).toBeNull();
      // Rien à comparer : rien n'est demandé.
      expect(simulator.callsTo("outdated_models")).toEqual([]);
      cleanup();
    }
  });

  it("Update queues the download under the installed id, shows the progress, then leaves", async () => {
    // La réponse peut écrire la quantification autrement que l'identifiant
    // installé : le téléchargement suit l'identifiant installé.
    const { simulator, backend, hub } = renderBanner({
      prepare: (backend) => void backend.outdated.add(QWEN.toLowerCase()),
    });
    await screen.findByText(AVAILABLE);
    await userEvent.click(update());
    expect(await screen.findByText(`Updating ${QWEN_NAME}…`)).toBeTruthy();
    expect(simulator.callsTo("hub_download")).toEqual([
      { request: { model: QWEN, repo: null, label: null } },
    ]);
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();

    await hub.progress(QWEN, 250, { total: 1000 });
    expect(await screen.findByText(`Updating ${QWEN_NAME}… 25%`)).toBeTruthy();

    await hub.finish(QWEN);
    await waitFor(() => expect(banner()).toBeNull());
    // Le modèle installé est remplacé, pas doublé.
    expect(backend.models.filter((name) => name === QWEN)).toEqual([QWEN]);
  });

  it("Update waits for the end of the reply being written", async () => {
    const { simulator, rerender } = renderBanner({ isStreaming: true });
    await screen.findByText(AVAILABLE);
    expect(update()).toHaveProperty("disabled", true);
    await userEvent.click(update());
    expect(simulator.callsTo("hub_download")).toEqual([]);
    rerender(QWEN, false);
    expect(update()).toHaveProperty("disabled", false);
  });

  it("Dismiss hides it for that model only, until the end of the session", async () => {
    const other = OTHER;
    const { rerender } = renderBanner({
      prepare: (backend) => {
        backend.outdated.add(QWEN);
        backend.outdated.add(other);
      },
    });
    await screen.findByText(AVAILABLE);
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(banner()).toBeNull();
    // Un autre modèle périmé garde son bandeau.
    rerender(other);
    expect(
      await screen.findByText(
        "A newer version of Qwen3.5-0.8B:Q4_K_M is available",
      ),
    ).toBeTruthy();
    // De retour au premier : toujours fermé, quantification écrite
    // autrement aussi.
    rerender(QWEN);
    await waitFor(() => expect(banner()).toBeNull());
    rerender(QWEN.replace("Q4_K_M", "q4_k_m"));
    expect(banner()).toBeNull();
  });

  it("a failed update says why, and Update tries again", async () => {
    const { simulator, hub } = renderBanner();
    await screen.findByText(AVAILABLE);
    await userEvent.click(update());
    await screen.findByText(`Updating ${QWEN_NAME}…`);
    await hub.fail(QWEN, "the file is gone from the repository");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't update the model: the file is gone from the repository",
    );
    expect(screen.getByText(AVAILABLE)).toBeTruthy();
    await userEvent.click(update());
    await screen.findByText(`Updating ${QWEN_NAME}…`);
    expect(simulator.callsTo("hub_download")).toHaveLength(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("a download refused by the queue is reported", async () => {
    const { hub } = renderBanner();
    hub.failures.set("hub_download", {
      kind: "invalid",
      message: "The models folder is being changed.",
    });
    await screen.findByText(AVAILABLE);
    await userEvent.click(update());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't update the model: The models folder is being changed.",
    );
    expect(update()).toHaveProperty("disabled", false);
  });
});

describe("the banner in the app", () => {
  it("is shown above the composer of a new chat, for the selected model", async () => {
    const { simulator } = renderHub(
      "/c/new",
      ({ backend }) => void backend.outdated.add(QWEN),
      INSTALLED,
    );
    const shown = await screen.findByTestId("stale-model");
    expect(shown.textContent).toContain(AVAILABLE);
    // Au-dessus de la zone de saisie.
    const field = screen.getByRole("textbox");
    expect(
      shown.compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(simulator.callsTo("outdated_models")).toEqual([{ models: [QWEN] }]);
  });

  it("is not shown for a selected model that is not installed", async () => {
    const missing = "unsloth/gemma-3-4b-it-GGUF:Q4_K_M";
    const { simulator } = renderHub(
      "/c/new",
      ({ backend }) => void backend.outdated.add(missing),
      { ...INSTALLED, selectedModel: missing },
    );
    await screen.findByRole("textbox");
    await waitFor(() =>
      expect(simulator.callsTo("list_models").length).toBeGreaterThan(0),
    );
    expect(banner()).toBeNull();
    expect(simulator.callsTo("outdated_models")).toEqual([]);
  });
});
