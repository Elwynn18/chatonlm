// Onglet Polish face au simulateur : les modèles conseillés sont des
// identifiants de Hugging Face, lus installés dans la base locale
// (hub_installed) même quand Ollama ne les connaît pas, ou sous leur nom
// `hf.co/…` chez Ollama ; Download les envoie tels quels au ModelHub.

import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { HubInstalled } from "@/bindings/HubInstalled";
import { fakeBackend } from "@/test/backend";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const GEMMA = "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M";
const QWEN = "Qwen/Qwen3-8B-GGUF:Q4_K_M";

function local(model: string): HubInstalled {
  return {
    model,
    size: 1,
    modifiedAt: null,
    parameterSize: "",
    quantizationLevel: "Q4_K_M",
    family: "",
    capabilities: ["completion"],
    repo: model.split(":")[0],
  };
}

/** L'onglet Polish ; `installed` : base locale ; `ollama` : list_models. */
function renderPolish(installed: string[], ollama: string[] = []) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.models.splice(0, backend.models.length, ...ollama);
  simulator.handle("hub_installed", () => installed.map(local));
  renderApp("/hub/polish");
  return simulator;
}

async function card(model: string) {
  return screen.findByRole("listitem", { name: model });
}

describe("Polish models", () => {
  it("a model of the local base is installed, the others downloadable", async () => {
    renderPolish([GEMMA]);
    const gemma = await card(GEMMA);
    expect(
      await within(gemma).findByRole("button", { name: "Use for polish" }),
    ).toBeTruthy();
    const qwen = await card(QWEN);
    expect(
      await within(qwen).findByRole("button", { name: "Download" }),
    ).toBeTruthy();
  });

  it("Ollama's hf.co name of the same model counts too", async () => {
    renderPolish([], [`hf.co/${QWEN}`]);
    const qwen = await card(QWEN);
    expect(
      await within(qwen).findByRole("button", { name: "Use for polish" }),
    ).toBeTruthy();
  });

  it("Download sends the Hugging Face id to the ModelHub", async () => {
    const simulator = renderPolish([]);
    const gemma = await card(GEMMA);
    await userEvent.click(
      await within(gemma).findByRole("button", { name: "Download" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("hub_download")).toEqual([
        { request: { model: GEMMA, repo: null, label: null } },
      ]),
    );
  });
});
