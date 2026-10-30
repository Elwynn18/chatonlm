// Proposition de téléchargement venue de VS Code, face au simulateur : rien
// n'est téléchargé sans l'accord de l'utilisateur ; « Download » lance le
// téléchargement par le chemin du ModelHub ; « Cancel » n'envoie rien ; une
// licence d'usage non commercial est dite.

import { configure, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fakeBackend } from "@/test/backend";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MODEL = "qwen2.5-coder:1.5b-base";

async function offered(
  path: string,
  failing = false,
  model = MODEL,
  commercialUse = true,
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, { selectedModel: "qwen3:0.6b" });
  if (failing) {
    backend.hub.failures.set("hub_download", {
      kind: "ollama",
      message: "can't connect to Ollama",
    });
  }
  renderApp(path);
  // Écouteur posé par la route racine.
  await waitFor(() =>
    expect(simulator.listening("vscode-offer-pull")).toBeGreaterThan(0),
  );
  await simulator.emitEvent("vscode-offer-pull", { model, commercialUse });
  await screen.findByRole("alertdialog", { name: `Download ${model}?` });
  return simulator;
}

describe("download offered by VS Code", () => {
  it("downloads only once the user accepts", async () => {
    const simulator = await offered("/settings");
    expect(simulator.callsTo("hub_download")).toEqual([]);
    expect(
      screen.getByText(
        `VS Code asks for ${MODEL} to suggest code as you type. The model runs on this computer; nothing you type leaves it.`,
      ),
    ).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    await waitFor(() =>
      expect(simulator.callsTo("hub_download")).toEqual([
        { request: { model: MODEL, repo: null, label: null } },
      ]),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByText(/non-commercial/)).toBeNull();
  });

  it("says when the license allows non-commercial use only", async () => {
    const simulator = await offered(
      "/settings",
      false,
      "qwen2.5-coder:3b-base",
      false,
    );
    expect(
      screen.getByText("Its license allows non-commercial use only."),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "VS Code asks for qwen2.5-coder:3b-base to suggest code as you type. The model runs on this computer; nothing you type leaves it.",
      ),
    ).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    await waitFor(() =>
      expect(simulator.callsTo("hub_download")).toEqual([
        {
          request: { model: "qwen2.5-coder:3b-base", repo: null, label: null },
        },
      ]),
    );
  });

  it("sends nothing when cancelled", async () => {
    const simulator = await offered("/c/new");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(simulator.callsTo("hub_download")).toEqual([]);
  });

  it("says when the download couldn't start", async () => {
    await offered("/hub", true);
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    await screen.findByText("can't connect to Ollama");
    await userEvent.click(screen.getByRole("button", { name: "OK" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });
});
