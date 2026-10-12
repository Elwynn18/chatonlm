// Onglet Installed face au simulateur : lignes, Use in chat, suppression
// confirmée (focus rendu), suppressions rapprochées, échecs, liste vide.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { modelsKey } from "@/hooks/useModels";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const SMOL = "hf.co/unsloth/Qwen3.5-9B-GGUF:Q4_K_M";

const MIB = 1024 ** 2;

/** Tailles fixées : 6 GB, 512 MB et 1 GB. */
function sizes(hub: import("@/test/hub").FakeHub) {
  hub.installedInfo.set(SMOL.toLowerCase(), { size: 6 * 1024 * MIB });
  hub.installedInfo.set("qwen3:0.6b", { size: 512 * MIB });
  hub.installedInfo.set("qwen3.5:0.8b", { size: 1024 * MIB });
}

const rows = () => screen.getAllByTestId("installed-model");
const row = (text: string) =>
  rows().find((element) => element.textContent?.includes(text))!;

describe("Installed", () => {
  it("lists the models with their size, tag and details", async () => {
    renderHub("/hub/installed", ({ backend, hub }) => {
      backend.models.unshift(SMOL);
      sizes(hub);
    });
    expect(
      await screen.findByRole("heading", { name: "3 models · 7.5 GB" }),
    ).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Installed" })
        .getAttribute("aria-current"),
    ).toBe("page");
    const hf = row("Qwen3.5-9B");
    const title = within(hf).getByRole("link", { name: "Qwen3.5-9B" });
    expect(title.getAttribute("href")).toBe(
      "/hub/model/unsloth/Qwen3.5-9B-GGUF",
    );
    expect(
      within(hf).getByText("Q4_K_M", { selector: "span.font-mono" }),
    ).toBeTruthy();
    expect(hf.textContent).toContain("unsloth");
    expect(hf.textContent).toContain("6.0 GB");
    expect(
      within(hf)
        .getAllByRole("img")
        .map((icon) => icon.getAttribute("aria-label")),
    ).toEqual(["Tools", "Reasoning", "Vision"]);
    const qwen = row("qwen3");
    expect(within(qwen).queryByRole("link", { name: "qwen3" })).toBeNull();
    expect(within(qwen).getByText("0.6b")).toBeTruthy();
    expect(qwen.textContent).toMatch(/512 MB·751\.63M·Q4_K_M·Modified/);
  });

  it("Use in chat selects the model and opens a new chat", async () => {
    const { simulator, app } = renderHub("/hub/installed");
    await screen.findByRole("heading", { name: /models ·/ });
    await userEvent.click(
      within(row("qwen3.5")).getByRole("button", { name: "Use in chat" }),
    );
    await waitFor(() => expect(app.path()).toBe("/c/new"));
    expect(simulator.callsTo("update_settings")).toEqual([
      { patch: { selectedModel: "qwen3.5:0.8b" } },
    ]);
  });

  it("deletes a model after confirmation; the picker reads its models again", async () => {
    const { simulator, app } = renderHub(
      "/hub/installed",
      ({ backend, hub }) => {
        backend.models.unshift(SMOL);
        sizes(hub);
      },
    );
    await screen.findByRole("heading", { name: "3 models · 7.5 GB" });
    app.queryClient.setQueryData(modelsKey, []);

    await userEvent.click(
      screen.getByRole("button", { name: "Delete Qwen3.5-9B:Q4_K_M" }),
    );
    const dialog = screen.getByRole("alertdialog", {
      name: "Delete Qwen3.5-9B:Q4_K_M?",
    });
    expect(dialog.textContent).toContain(
      "This removes the model from Ollama. You can download it again later.",
    );
    // Annuler : rien n'est supprimé.
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("hub_delete_model")).toEqual([]);

    await userEvent.click(
      screen.getByRole("button", { name: "Delete Qwen3.5-9B:Q4_K_M" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(simulator.callsTo("hub_delete_model")).toEqual([{ model: SMOL }]);
    await screen.findByRole("heading", { name: "2 models · 1.5 GB" });
    expect(app.queryClient.getQueryState(modelsKey)?.isInvalidated).toBe(true);
  });

  it("a failed deletion is shown on its row", async () => {
    renderHub("/hub/installed", ({ hub }) => {
      hub.failures.set("hub_delete_model", {
        kind: "ollama",
        message: "Can't reach Ollama. Make sure it's running, then try again.",
      });
    });
    await screen.findByRole("heading", { name: /models ·/ });
    await userEvent.click(
      screen.getByRole("button", { name: "Delete qwen3:0.6b" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    const alert = await within(row("0.6b")).findByRole("alert");
    expect(alert.textContent).toBe(
      "Can't reach Ollama. Make sure it's running, then try again.",
    );
    expect(rows()).toHaveLength(2);
  });

  it("two deletions close together: each row keeps its state and its failure", async () => {
    const answers = new Map<
      string,
      { resolve: (value: null) => void; reject: (error: unknown) => void }
    >();
    const { simulator, backend } = renderHub("/hub/installed");
    simulator.handle(
      "hub_delete_model",
      (args) =>
        new Promise((resolve, reject) =>
          answers.set(args.model as string, { resolve, reject }),
        ),
    );
    await screen.findByRole("heading", { name: /models ·/ });
    for (const name of ["qwen3:0.6b", "qwen3.5:0.8b"]) {
      await userEvent.click(
        screen.getByRole("button", { name: `Delete ${name}` }),
      );
      await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    }
    await waitFor(() => expect(answers.size).toBe(2));
    // Les deux lignes restent occupées tant que leur suppression dure.
    expect(row("0.6b").getAttribute("aria-busy")).toBe("true");
    expect(row("0.8b").getAttribute("aria-busy")).toBe("true");
    expect(
      within(row("0.6b")).getByRole("button", { name: "Delete qwen3:0.6b" }),
    ).toHaveProperty("disabled", true);

    // La première échoue, la seconde réussit.
    await act(async () =>
      answers.get("qwen3:0.6b")!.reject({
        kind: "ollama",
        message: "Can't reach Ollama. Make sure it's running, then try again.",
      }),
    );
    backend.models.splice(backend.models.indexOf("qwen3.5:0.8b"), 1);
    await act(async () => answers.get("qwen3.5:0.8b")!.resolve(null));
    const alert = await within(row("0.6b")).findByRole("alert");
    expect(alert.textContent).toBe(
      "Can't reach Ollama. Make sure it's running, then try again.",
    );
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(row("0.6b").getAttribute("aria-busy")).toBeNull();
  });

  it("the keyboard focus comes back after the dialog", async () => {
    const { simulator } = renderHub("/hub/installed");
    const heading = await screen.findByRole("heading", { name: /models ·/ });
    const trash = screen.getByRole("button", { name: "Delete qwen3:0.6b" });
    act(() => trash.focus());
    await userEvent.keyboard("{Enter}");
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Delete" }),
    );
    // Échap : retour à la corbeille.
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(document.activeElement).toBe(trash);
    // Delete : au titre de la liste, la ligne va disparaître.
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard("{Enter}");
    expect(simulator.callsTo("hub_delete_model")).toEqual([
      { model: "qwen3:0.6b" },
    ]);
    expect(document.activeElement).toBe(heading);
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(document.activeElement).toBe(heading);
  });

  it("an embedding model can't be used in a chat", async () => {
    renderHub("/hub/installed", ({ backend, hub }) => {
      backend.models.unshift("nomic-embed-text:latest");
      hub.installedInfo.set("nomic-embed-text:latest", {
        capabilities: ["embedding"],
      });
    });
    await screen.findByRole("heading", { name: /models ·/ });
    const embed = row("nomic-embed-text");
    expect(within(embed).getByText("Embedding")).toBeTruthy();
    expect(
      within(embed).queryByRole("button", { name: "Use in chat" }),
    ).toBeNull();
  });

  it("without models: No models yet, Browse models", async () => {
    const { app } = renderHub("/hub/installed", ({ backend }) => {
      backend.models.length = 0;
    });
    expect(await screen.findByText("No models yet")).toBeTruthy();
    await userEvent.click(screen.getByRole("link", { name: "Browse models" }));
    await waitFor(() => expect(app.path()).toBe("/hub"));
  });

  it("Ollama unreachable: the error and Retry", async () => {
    const { hub } = renderHub("/hub/installed", ({ hub }) => {
      hub.failures.set("hub_installed", {
        kind: "ollama",
        message: "Can't reach Ollama. Make sure it's running, then try again.",
      });
    });
    expect(
      await screen.findByText(
        "Can't reach Ollama. Make sure it's running, then try again.",
      ),
    ).toBeTruthy();
    hub.failures.clear();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("heading", { name: /models ·/ }),
    ).toBeTruthy();
  });
});
