// Onglet Downloads face au simulateur : ordre, états, Cancel, Use in chat,
// Dismiss, Try again ; barre latérale : entrée ModelHub et indicateur.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { HubDownload } from "@/bindings/HubDownload";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { hubInstalledKey } from "@/hooks/useHub";
import type { FakeBackend } from "@/test/backend";
import { PARAKEET, WHISPER_TINY } from "@/test/dictation";
import { type FakeHub, GIB } from "@/test/hub";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function entry(model: string, fields: Partial<HubDownload> = {}): HubDownload {
  const repo = /^hf\.co\/([^:]+)/.exec(model)?.[1] ?? null;
  return {
    model,
    repo,
    label: model.split(":").at(-1) ?? null,
    state: "downloading",
    completed: 0,
    total: 0,
    bytesPerSecond: null,
    error: null,
    startedAt: 1,
    finishedAt: null,
    ...fields,
  };
}

const GEMMA = "hf.co/google/gemma-4-12B-it-qat-q4_0-gguf:Q4_0";
const QWEN = "hf.co/unsloth/Qwen3.5-9B-GGUF:Q4_K_M";
const PHI = "hf.co/unsloth/Phi-4-mini-reasoning-GGUF:Q8_0";
const SMOL = "hf.co/ggml-org/SmolLM3-3B-GGUF:Q4_K_M";
const LFM = "hf.co/LiquidAI/LFM2-1.2B-GGUF:Q4_K_M";

/**
 * Un de chaque : terminés à des dates différentes, actifs, en file ; le
 * modèle téléchargé est installé.
 */
function variety({ backend, hub }: { backend: FakeBackend; hub: FakeHub }) {
  backend.models.unshift(PHI);
  hub.downloads.push(
    entry(PHI, {
      state: "done",
      completed: 4 * GIB,
      total: 4 * GIB,
      finishedAt: 100,
    }),
    entry(QWEN, {
      state: "downloading",
      completed: 2 * GIB,
      total: 5 * GIB,
      bytesPerSecond: 50 * 1024 ** 2,
    }),
    entry(SMOL, {
      state: "failed",
      error: "Hugging Face didn't answer in time. Try again.",
      finishedAt: 200,
    }),
    entry(GEMMA, { state: "queued" }),
    entry(LFM, { state: "cancelled", finishedAt: 50 }),
  );
}

const rows = () => screen.getAllByTestId("download");
const row = (title: string) =>
  rows().find((element) => element.textContent?.includes(title))!;

describe("Downloads", () => {
  it("without downloads: the empty text", async () => {
    renderHub("/hub/downloads");
    expect(await screen.findByText("No downloads yet.")).toBeTruthy();
    expect(
      screen.getByText("Models you download from ModelHub appear here."),
    ).toBeTruthy();
  });

  it("lists active downloads first, then finished ones, newest first", async () => {
    renderHub("/hub/downloads", variety);
    await screen.findAllByTestId("download");
    expect(rows().map((element) => element.getAttribute("data-state"))).toEqual(
      ["downloading", "queued", "failed", "done", "cancelled"],
    );
    expect(screen.getByTestId("active-downloads").textContent).toBe("2");

    const qwen = rows()[0];
    expect(qwen.textContent).toContain(
      "2.0 GB of 5.0 GB · 50 MB/s · 1 min left",
    );
    expect(
      within(qwen).getByRole("progressbar").getAttribute("aria-valuenow"),
    ).toBe("40");
    expect(
      within(qwen)
        .getByRole("link", { name: "Qwen3.5-9B" })
        .getAttribute("href"),
    ).toBe("/hub/model/unsloth/Qwen3.5-9B-GGUF");
    expect(within(rows()[1]).getByText("Waiting…")).toBeTruthy();
    expect(within(rows()[2]).getByRole("alert").textContent).toBe(
      "Hugging Face didn't answer in time. Try again.",
    );
    expect(rows()[3].textContent).toContain("Installed · 4.0 GB");
    expect(rows()[4].textContent).toContain("Cancelled");
    // Actions de chaque état.
    const buttons = (element: HTMLElement) =>
      within(element)
        .getAllByRole("button")
        .map((button) => button.textContent);
    expect(rows().map(buttons)).toEqual([
      ["Cancel"],
      ["Cancel"],
      ["Try again", "Dismiss"],
      ["Use in chat", "Dismiss"],
      ["Try again", "Dismiss"],
    ]);
  });

  it("Cancel, Dismiss and Try again call the backend", async () => {
    const { simulator } = renderHub("/hub/downloads", variety);
    await screen.findAllByTestId("download");
    await userEvent.click(
      within(row("Qwen3.5-9B")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("hub_cancel_download")).toEqual([{ model: QWEN }]);
    await waitFor(() =>
      expect(row("Qwen3.5-9B").getAttribute("data-state")).toBe("cancelled"),
    );

    await userEvent.click(
      within(row("Phi-4-mini-reasoning")).getByRole("button", {
        name: "Dismiss",
      }),
    );
    expect(simulator.callsTo("hub_dismiss_download")).toEqual([{ model: PHI }]);
    await waitFor(() => expect(row("Phi-4-mini-reasoning")).toBeUndefined());

    await userEvent.click(
      within(row("SmolLM3-3B")).getByRole("button", { name: "Try again" }),
    );
    expect(simulator.callsTo("hub_download")).toEqual([
      {
        request: {
          model: SMOL,
          repo: "ggml-org/SmolLM3-3B-GGUF",
          label: "Q4_K_M",
        },
      },
    ]);
    await waitFor(() =>
      expect(row("SmolLM3-3B").getAttribute("data-state")).not.toBe("failed"),
    );
  });

  it("Use in chat on a finished download", async () => {
    const { simulator, app } = renderHub("/hub/downloads", variety);
    await screen.findAllByTestId("download");
    await userEvent.click(screen.getByRole("button", { name: "Use in chat" }));
    await waitFor(() => expect(app.path()).toBe("/c/new"));
    expect(simulator.callsTo("update_settings")).toEqual([
      { patch: { selectedModel: PHI } },
    ]);
  });

  it("a downloaded model deleted since: No longer installed, Download again", async () => {
    const { simulator, app } = renderHub("/hub/downloads", ({ hub }) => {
      hub.downloads.push(
        entry(PHI, {
          state: "done",
          completed: 4 * GIB,
          total: 4 * GIB,
          finishedAt: 100,
        }),
      );
    });
    await waitFor(() =>
      expect(row("Phi-4-mini-reasoning").textContent).toContain(
        "No longer installed",
      ),
    );
    expect(row("Phi-4-mini-reasoning").textContent).not.toContain(
      "Installed ·",
    );
    expect(
      within(row("Phi-4-mini-reasoning"))
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Download again", "Dismiss"]);
    // Relecture des installés en cours : toujours pas installé.
    simulator.handle("hub_installed", () => new Promise(() => {}));
    void app.queryClient.invalidateQueries({ queryKey: hubInstalledKey });
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(row("Phi-4-mini-reasoning").textContent).toContain(
      "No longer installed",
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Download again" }),
    );
    expect(simulator.callsTo("hub_download")).toEqual([
      {
        request: {
          model: PHI,
          repo: "unsloth/Phi-4-mini-reasoning-GGUF",
          label: "Q8_0",
        },
      },
    ]);
  });

  it("follows the events of the backend", async () => {
    const { hub } = renderHub("/hub/downloads", ({ hub }) => {
      hub.downloads.push(entry(QWEN, { state: "starting" }));
    });
    expect(await screen.findByText("Starting…")).toBeTruthy();
    await hub.progress(QWEN, GIB, { total: 4 * GIB });
    expect(await screen.findByText("1.0 GB of 4.0 GB")).toBeTruthy();
    await hub.finish(QWEN);
    expect(await screen.findByText("Installed · 4.0 GB")).toBeTruthy();
  });
});

describe("Downloads of speech models", () => {
  function speech(
    id: string,
    name: string,
    fields: Partial<SpeechDownload> = {},
  ): SpeechDownload {
    return {
      id,
      name,
      state: "downloading",
      completedBytes: 0,
      totalBytes: 512 * 1024 ** 2,
      error: null,
      ...fields,
    };
  }

  const speechRows = () => screen.getAllByTestId("speech-download");

  it("only speech models: their own section, no empty state", async () => {
    renderHub("/hub/downloads", ({ backend }) => {
      backend.dictation.install(PARAKEET);
      backend.dictation.downloads.push(
        speech(PARAKEET, "Parakeet TDT 0.6B v3", { state: "done" }),
        speech(WHISPER_TINY, "Whisper Tiny", {
          completedBytes: 128 * 1024 ** 2,
        }),
        speech("whisper-base", "Whisper Base", {
          state: "failed",
          error: "Hugging Face didn't answer in time.",
        }),
      );
    });
    const section = await screen.findByRole("region", {
      name: "Speech models",
    });
    expect(screen.queryByText("No downloads yet.")).toBeNull();
    expect(screen.queryByRole("list", { name: "Downloads" })).toBeNull();
    // Actifs d'abord.
    expect(
      within(section)
        .getAllByTestId("speech-download")
        .map((row) => row.getAttribute("data-state")),
    ).toEqual(["downloading", "done", "failed"]);
    const [tiny, parakeet, base] = speechRows();
    expect(
      within(tiny)
        .getByRole("progressbar", { name: "Downloading Whisper Tiny" })
        .getAttribute("aria-valuenow"),
    ).toBe("25");
    expect(tiny.textContent).toContain("128 MB of 512 MB");
    expect(parakeet.textContent).toContain("Installed · 512 MB");
    expect(within(base).getByRole("alert").textContent).toBe(
      "Hugging Face didn't answer in time.",
    );
    expect(
      within(tiny)
        .getByRole("link", { name: "Whisper Tiny" })
        .getAttribute("href"),
    ).toBe("/hub/transcription");
    const buttons = (row: HTMLElement) =>
      within(row)
        .getAllByRole("button")
        .map((button) => button.textContent);
    expect(speechRows().map(buttons)).toEqual([
      ["Cancel"],
      ["Dismiss"],
      ["Try again", "Dismiss"],
    ]);
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");
  });

  it("a model deleted since its download: No longer installed, Download again", async () => {
    const { simulator } = renderHub("/hub/downloads", ({ backend }) => {
      backend.dictation.downloads.push(
        speech(PARAKEET, "Parakeet TDT 0.6B v3", { state: "done" }),
      );
    });
    const row = await screen.findByTestId("speech-download");
    await within(row).findByText("No longer installed");
    expect(row.textContent).not.toContain("Installed ·");
    expect(
      within(row)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Download again", "Dismiss"]);
    await userEvent.click(
      within(row).getByRole("button", { name: "Download again" }),
    );
    expect(simulator.callsTo("speech_download")).toEqual([{ id: PARAKEET }]);
  });

  it("under the pulls; Cancel, Try again and Dismiss call the backend", async () => {
    const { simulator, backend } = renderHub("/hub/downloads", (setup) => {
      variety(setup);
      setup.backend.dictation.downloads.push(
        speech(WHISPER_TINY, "Whisper Tiny", { state: "queued" }),
        speech("whisper-base", "Whisper Base", { state: "cancelled" }),
      );
    });
    await screen.findAllByTestId("download");
    const section = screen.getByRole("region", { name: "Speech models" });
    // Après la liste des pulls.
    expect(
      screen
        .getByRole("list", { name: "Downloads" })
        .compareDocumentPosition(section) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.getByTestId("active-downloads").textContent).toBe("3");
    const row = (name: string) =>
      speechRows().find((element) => element.textContent?.includes(name))!;
    expect(within(row("Whisper Tiny")).getByText("Waiting…")).toBeTruthy();
    expect(row("Whisper Base").textContent).toContain("Cancelled");

    await userEvent.click(
      within(row("Whisper Tiny")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("speech_cancel_download")).toEqual([
      { id: WHISPER_TINY },
    ]);
    await waitFor(() =>
      expect(row("Whisper Tiny").getAttribute("data-state")).toBe("cancelled"),
    );
    await userEvent.click(
      within(row("Whisper Base")).getByRole("button", { name: "Try again" }),
    );
    expect(simulator.callsTo("speech_download")).toEqual([
      { id: "whisper-base" },
    ]);
    await waitFor(() =>
      expect(row("Whisper Base").getAttribute("data-state")).toBe("queued"),
    );
    await userEvent.click(
      within(row("Whisper Tiny")).getByRole("button", { name: "Dismiss" }),
    );
    expect(simulator.callsTo("speech_dismiss_download")).toEqual([
      { id: WHISPER_TINY },
    ]);
    await waitFor(() => expect(row("Whisper Tiny")).toBeUndefined());

    // Événements du moteur.
    await backend.dictation.progress("whisper-base", 64 * 1024 ** 2);
    await waitFor(() =>
      expect(row("Whisper Base").textContent).toContain("64 MB of 141 MB"),
    );
  });
});

describe("Sidebar", () => {
  /** Barre latérale ouverte (son état dure toute la session). */
  async function openSidebar() {
    const toggle = await screen.findByRole("button", {
      name: /^(Show|Hide) sidebar$/,
    });
    if (toggle.getAttribute("aria-label") === "Show sidebar") {
      await userEvent.click(toggle);
    }
    await screen.findByRole("link", { name: "ModelHub" });
  }

  it("has a ModelHub entry between Study and Settings, current on its pages", async () => {
    renderHub("/hub/installed");
    await openSidebar();
    const nav = screen.getByRole("link", { name: "ModelHub" }).closest("nav")!;
    expect(
      within(nav)
        .getAllByRole("link")
        .map((link) => link.textContent),
    ).toEqual(["Chat", "Proofread", "Study", "ModelHub", "Settings"]);
    const hub = within(nav).getByRole("link", { name: "ModelHub" });
    expect(hub.getAttribute("href")).toBe("/hub");
    const classes = (link: HTMLElement) => link.className.split(" ");
    expect(classes(hub)).toContain("bg-accent");
    expect(
      classes(within(nav).getByRole("link", { name: "Settings" })),
    ).not.toContain("bg-accent");
  });

  it("shows the active downloads under ModelHub, from any page", async () => {
    const { hub } = renderHub("/settings", ({ hub }) => {
      hub.downloads.push(
        entry(QWEN, { completed: 1 * GIB, total: 4 * GIB }),
        entry(GEMMA, { state: "queued" }),
        entry(PHI, {
          state: "done",
          completed: GIB,
          total: GIB,
          finishedAt: 5,
        }),
      );
    });
    await openSidebar();
    const indicator = await screen.findByTestId("download-indicator");
    expect(indicator.getAttribute("href")).toBe("/hub/downloads");
    expect(indicator.textContent).toBe("Downloading 2 · 25%");
    const bar = within(indicator).getByRole("progressbar", {
      name: "Downloads",
    });
    expect(bar.getAttribute("aria-valuenow")).toBe("25");

    await hub.progress(QWEN, 3 * GIB);
    await waitFor(() =>
      expect(indicator.textContent).toBe("Downloading 2 · 75%"),
    );
    await hub.fail(QWEN, "Connection lost");
    await hub.setState(GEMMA, "cancelled");
    await waitFor(() =>
      expect(screen.queryByTestId("download-indicator")).toBeNull(),
    );
    await act(() => Promise.resolve());
  });
});
