// Onglets Transcription et Polish du ModelHub face au simulateur : cartes,
// licences et attribution, téléchargement (progression, Cancel, échec),
// Use / In use, suppression confirmée ; modèles de nettoyage tirés par le
// pull du ModelHub ; onglets et badge de Downloads.

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
import { PARAKEET, WHISPER_TINY } from "@/test/dictation";
import { GIB } from "@/test/hub";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 ** 2;

const cards = () => screen.getAllByTestId("speech-model");
const card = (name: string) =>
  screen.getByRole("listitem", { name }) as HTMLElement;

const patches = (simulator: ReturnType<typeof renderHub>["simulator"]) =>
  simulator.callsTo("update_dictation_settings").map((args) => args.patch);

describe("Transcription tab", () => {
  it("one card per model: engine, recommendation, size, languages, license and source", async () => {
    renderHub("/hub/transcription");
    await screen.findAllByTestId("speech-model");
    expect(
      screen.getByText("Speech models run inside ChatonLM, not in Ollama.", {
        exact: false,
      }),
    ).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Dictation settings" })
        .getAttribute("href"),
    ).toBe("/settings?section=dictation");
    expect(
      cards().map(
        (element) => within(element).getByRole("heading").textContent,
      ),
    ).toEqual([
      "Parakeet TDT 0.6B v3",
      "Whisper Tiny",
      "Whisper Base",
      "Whisper Small",
      "Whisper Large v3 Turbo",
    ]);
    const parakeet = card("Parakeet TDT 0.6B v3");
    expect(within(parakeet).getByText("Parakeet")).toBeTruthy();
    expect(within(parakeet).getByText("Recommended")).toBeTruthy();
    expect(parakeet.textContent).toContain("640 MB");
    expect(parakeet.textContent).toContain("25 European languages");
    // CC BY 4.0 : licence et œuvre d'origine visibles et liées.
    expect(
      within(parakeet)
        .getByRole("link", { name: "CC BY 4.0" })
        .getAttribute("href"),
    ).toBe("https://creativecommons.org/licenses/by/4.0/");
    expect(parakeet.textContent).toContain(
      "Based on nvidia/parakeet-tdt-0.6b-v3, licensed under CC BY 4.0.",
    );
    expect(
      within(parakeet)
        .getByRole("link", { name: "nvidia/parakeet-tdt-0.6b-v3" })
        .getAttribute("href"),
    ).toBe("https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3");
    expect(
      within(parakeet)
        .getByRole("link", { name: "istupakov/parakeet-tdt-0.6b-v3-onnx" })
        .getAttribute("href"),
    ).toBe("https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx");

    const tiny = card("Whisper Tiny");
    expect(within(tiny).queryByText("Recommended")).toBeNull();
    expect(
      within(tiny).getByRole("link", { name: "MIT" }).getAttribute("href"),
    ).toBe("https://github.com/openai/whisper/blob/main/LICENSE");
    expect(tiny.textContent).not.toContain("Based on");
    expect(
      cards().map((element) =>
        within(element)
          .getAllByRole("button")
          .map((button) => button.textContent),
      ),
    ).toEqual([
      ["Download"],
      ["Download"],
      ["Download"],
      ["Download"],
      ["Download"],
    ]);
  });

  it("Download: progress, Cancel, then Use once it is installed", async () => {
    const { simulator, backend } = renderHub("/hub/transcription");
    const dictation = backend.dictation;
    await screen.findAllByTestId("speech-model");
    await userEvent.click(
      within(card("Whisper Tiny")).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("speech_download")).toEqual([
      { id: WHISPER_TINY },
    ]);
    // Liste relue : en file, badge de l'onglet Downloads.
    await within(card("Whisper Tiny")).findByText("Waiting…");
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    await dictation.progress(WHISPER_TINY, 37 * MIB);
    const bar = await within(card("Whisper Tiny")).findByRole("progressbar", {
      name: "Downloading Whisper Tiny",
    });
    await waitFor(() => expect(bar.getAttribute("aria-valuenow")).toBe("49"));
    expect(bar.getAttribute("aria-valuetext")).toBe("37 MB of 74 MB");
    await userEvent.click(
      within(card("Whisper Tiny")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("speech_cancel_download")).toEqual([
      { id: WHISPER_TINY },
    ]);
    await within(card("Whisper Tiny")).findByRole("button", {
      name: "Download",
    });

    await userEvent.click(
      within(card("Whisper Tiny")).getByRole("button", { name: "Download" }),
    );
    await within(card("Whisper Tiny")).findByText("Waiting…");
    await dictation.finish(WHISPER_TINY);
    // Modèles relus à la fin : installé.
    await within(card("Whisper Tiny")).findByRole("button", { name: "Use" });
    expect(screen.queryByTestId("active-downloads")).toBeNull();
    await userEvent.click(
      within(card("Whisper Tiny")).getByRole("button", { name: "Use" }),
    );
    expect(patches(simulator)).toEqual([{ speechModel: WHISPER_TINY }]);
    await within(card("Whisper Tiny")).findByText("In use");
    expect(
      within(card("Whisper Tiny")).queryByRole("button", { name: "Use" }),
    ).toBeNull();
  });

  it("a failed download says why, and can be started again", async () => {
    const { simulator, backend } = renderHub("/hub/transcription", () => {});
    const dictation = backend.dictation;
    await screen.findAllByTestId("speech-model");
    await userEvent.click(
      within(card("Whisper Base")).getByRole("button", { name: "Download" }),
    );
    await within(card("Whisper Base")).findByText("Waiting…");
    await dictation.fail(
      "whisper-base",
      "The file did not match its checksum. Try again.",
    );
    expect(
      (await within(card("Whisper Base")).findByRole("alert")).textContent,
    ).toBe("The file did not match its checksum. Try again.");
    await userEvent.click(
      within(card("Whisper Base")).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("speech_download")).toHaveLength(2);
  });

  it("a command that fails shows its error on the card", async () => {
    const { backend } = renderHub("/hub/transcription");
    backend.dictation.failures.set("speech_download", {
      kind: "internal",
      message: "no space left on device",
    });
    await screen.findAllByTestId("speech-model");
    await userEvent.click(
      within(card("Whisper Small")).getByRole("button", { name: "Download" }),
    );
    expect(
      (await within(card("Whisper Small")).findByRole("alert")).textContent,
    ).toBe("no space left on device");
  });

  it("Delete asks first, removes the files and puts the focus on the card title", async () => {
    const { simulator } = renderHub("/hub/transcription", ({ backend }) => {
      backend.dictation.install(PARAKEET, WHISPER_TINY);
      backend.dictation.settings.speechModel = PARAKEET;
    });
    await screen.findAllByTestId("speech-model");
    const parakeet = card("Parakeet TDT 0.6B v3");
    await within(parakeet).findByText("In use");
    await userEvent.click(
      within(parakeet).getByRole("button", {
        name: "Delete Parakeet TDT 0.6B v3",
      }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete Parakeet TDT 0.6B v3?",
    });
    expect(dialog.textContent).toContain(
      "Dictation won't work until you download it again or choose another speech model.",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("speech_delete_model")).toEqual([]);

    // Un modèle qui n'est pas utilisé.
    const tiny = card("Whisper Tiny");
    await userEvent.click(
      within(tiny).getByRole("button", { name: "Delete Whisper Tiny" }),
    );
    const confirm = await screen.findByRole("alertdialog", {
      name: "Delete Whisper Tiny?",
    });
    expect(confirm.textContent).toContain(
      "This removes the model files from this computer. You can download it again later.",
    );
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Delete" }),
    );
    expect(simulator.callsTo("speech_delete_model")).toEqual([
      { id: WHISPER_TINY },
    ]);
    expect(document.activeElement).toBe(
      within(tiny).getByRole("heading", { name: "Whisper Tiny" }),
    );
    await within(tiny).findByRole("button", { name: "Download" });
  });

  it("a refused delete shows why", async () => {
    const { backend } = renderHub("/hub/transcription", ({ backend }) => {
      backend.dictation.install(WHISPER_TINY);
    });
    backend.dictation.failures.set("speech_delete_model", {
      kind: "system",
      message: "permission denied",
    });
    await screen.findAllByTestId("speech-model");
    await userEvent.click(
      within(card("Whisper Tiny")).getByRole("button", {
        name: "Delete Whisper Tiny",
      }),
    );
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(
      (await within(card("Whisper Tiny")).findByRole("alert")).textContent,
    ).toBe("permission denied");
  });
});

describe("Transcription tab on an older processor", () => {
  it("a model this processor cannot run says so, without Download nor Use", async () => {
    const { simulator } = renderHub("/hub/transcription", ({ backend }) => {
      backend.dictation.install(WHISPER_TINY);
      for (const model of backend.dictation.models) {
        if (model.engine === "whisper") {
          model.missingCpuFeatures = ["AVX2", "FMA"];
        }
      }
    });
    await screen.findAllByTestId("speech-model");
    const note =
      "Not available on this computer: Whisper needs instructions its processor lacks (AVX2, FMA).";
    const base = card("Whisper Base");
    expect(base.textContent).toContain(note);
    expect(within(base).queryByRole("button", { name: "Download" })).toBeNull();
    // Installé (dossier venu d'une autre machine) : supprimable, pas
    // utilisable.
    const tiny = card("Whisper Tiny");
    expect(tiny.textContent).toContain(note);
    expect(within(tiny).queryByRole("button", { name: "Use" })).toBeNull();
    expect(
      within(tiny).getByRole("button", { name: "Delete Whisper Tiny" }),
    ).toBeTruthy();
    // Parakeet : rien ne change.
    const parakeet = card("Parakeet TDT 0.6B v3");
    expect(parakeet.textContent).not.toContain("Not available");
    await userEvent.click(
      within(parakeet).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("speech_download")).toEqual([{ id: PARAKEET }]);
  });
});

describe("Transcription tab without ONNX Runtime", () => {
  it("Parakeet says why it can't run, without Download nor Use; Whisper is unaffected", async () => {
    const { simulator } = renderHub("/hub/transcription", ({ backend }) => {
      backend.dictation.install(WHISPER_TINY);
      const parakeet = backend.dictation.models.find(
        (model) => model.id === PARAKEET,
      );
      if (parakeet) {
        parakeet.runtimeProblem = {
          kind: "missing",
          detail: "/usr/lib/chatonlm/libonnxruntime.so.1",
        };
      }
    });
    await screen.findAllByTestId("speech-model");
    const parakeet = card("Parakeet TDT 0.6B v3");
    expect(parakeet.textContent).toContain(
      "Not available: ONNX Runtime, which Parakeet needs, is missing (/usr/lib/chatonlm/libonnxruntime.so.1). Reinstall ChatonLM.",
    );
    expect(
      within(parakeet).queryByRole("button", { name: "Download" }),
    ).toBeNull();
    const tiny = card("Whisper Tiny");
    expect(tiny.textContent).not.toContain("ONNX Runtime");
    await userEvent.click(within(tiny).getByRole("button", { name: "Use" }));
    expect(patches(simulator)).toEqual([{ speechModel: WHISPER_TINY }]);
  });

  it("each reason has its own words", async () => {
    renderHub("/hub/transcription", ({ backend }) => {
      const parakeet = backend.dictation.models.find(
        (model) => model.id === PARAKEET,
      );
      if (parakeet) {
        parakeet.runtimeProblem = { kind: "loadFailed", detail: "GLIBC_2.38" };
      }
    });
    await screen.findAllByTestId("speech-model");
    expect(card("Parakeet TDT 0.6B v3").textContent).toContain(
      "Not available: ONNX Runtime, which Parakeet needs, could not be loaded (GLIBC_2.38).",
    );
  });
});

describe("Polish tab", () => {
  const polishCard = (model: string) =>
    screen.getByRole("listitem", { name: model }) as HTMLElement;

  it("lists the recommended models of Ollama, with their size and notes", async () => {
    renderHub("/hub/polish");
    await screen.findAllByTestId("polish-model");
    expect(
      screen.getByText(
        "A second model cleans up the transcript: punctuation, filler words, self-corrections. If its answer does not look like what you said, the raw transcript is copied instead.",
      ),
    ).toBeTruthy();
    expect(
      screen
        .getAllByTestId("polish-model")
        .map((element) => within(element).getByRole("heading").textContent),
    ).toEqual([
      "gemma3:4b",
      "qwen3:4b-instruct-2507-q4_K_M",
      "qwen3:8b",
      "gemma3:12b",
      "gemma3:1b",
    ]);
    expect(within(polishCard("gemma3:4b")).getByText("Recommended"));
    expect(within(polishCard("gemma3:4b")).getByText("3.3 GB"));
    expect(
      within(polishCard("qwen3:8b")).queryByText("Recommended"),
    ).toBeNull();
    expect(
      within(polishCard("gemma3:12b")).getByText(
        "Best quality, needs about 12 GB of GPU memory",
      ),
    );
    expect(
      within(polishCard("gemma3:1b")).getByText(
        "For small machines, less reliable",
      ),
    );
    expect(
      screen.getByText("Any installed model can be chosen in Settings.", {
        exact: false,
      }),
    ).toBeTruthy();
    await within(polishCard("gemma3:4b")).findByRole("button", {
      name: "Download",
    });
  });

  it("Download pulls the model through the ModelHub, then Use for polish", async () => {
    const { simulator, hub } = renderHub("/hub/polish");
    await screen.findAllByTestId("polish-model");
    await userEvent.click(
      await within(polishCard("qwen3:8b")).findByRole("button", {
        name: "Download",
      }),
    );
    expect(simulator.callsTo("hub_download")).toEqual([
      { request: { model: "qwen3:8b", repo: null, label: null } },
    ]);
    await hub.progress("qwen3:8b", GIB, { total: 4 * GIB });
    const bar = await within(polishCard("qwen3:8b")).findByRole("progressbar", {
      name: "Downloading qwen3:8b",
    });
    await waitFor(() => expect(bar.getAttribute("aria-valuenow")).toBe("25"));
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");
    await hub.finish("qwen3:8b");
    // Modèles d'Ollama relus à la fin du téléchargement.
    await userEvent.click(
      await within(polishCard("qwen3:8b")).findByRole("button", {
        name: "Use for polish",
      }),
    );
    expect(patches(simulator)).toEqual([{ polishModel: "qwen3:8b" }]);
    await within(polishCard("qwen3:8b")).findByText("In use");
  });

  it("an installed model in use says so; Cancel stops a pull", async () => {
    const { simulator } = renderHub(
      "/hub/polish",
      ({ backend, hub }) => {
        backend.dictation.settings.polishModel = "gemma3:4b";
        hub.downloads.push({
          model: "gemma3:12b",
          repo: null,
          label: null,
          state: "downloading",
          completed: GIB,
          total: 8 * GIB,
          bytesPerSecond: null,
          error: null,
          startedAt: 1,
          finishedAt: null,
        } satisfies HubDownload);
      },
      { models: ["gemma3:4b", "qwen3:0.6b"] },
    );
    await screen.findAllByTestId("polish-model");
    await within(polishCard("gemma3:4b")).findByText("In use");
    await userEvent.click(
      within(polishCard("gemma3:12b")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("hub_cancel_download")).toEqual([
      { model: "gemma3:12b" },
    ]);
    await within(polishCard("gemma3:12b")).findByRole("button", {
      name: "Download",
    });
  });
});

describe("ModelHub tabs", () => {
  it("two groups: Discover, Transcription, Polish, Voices, Images; then Installed, Downloads; the current one marked", async () => {
    const { app } = renderHub("/hub/transcription");
    const nav = await screen.findByRole("navigation", { name: "ModelHub" });
    const texts = (element: HTMLElement) =>
      within(element)
        .getAllByRole("link")
        .map((link) => link.textContent);
    expect(texts(nav)).toEqual([
      "Discover",
      "Transcription",
      "Polish",
      "Voices",
      "Images",
      "Installed",
      "Downloads",
    ]);
    expect(texts(within(nav).getByTestId("hub-tabs-browse"))).toEqual([
      "Discover",
      "Transcription",
      "Polish",
      "Voices",
      "Images",
    ]);
    expect(texts(within(nav).getByTestId("hub-tabs-local"))).toEqual([
      "Installed",
      "Downloads",
    ]);
    // Une ligne, les groupes aux deux bouts ; à la ligne si c'est étroit.
    const row = within(nav).getByTestId("hub-tabs-browse").parentElement!;
    expect(row.className.split(" ")).toEqual(
      expect.arrayContaining(["flex", "flex-wrap", "justify-between"]),
    );
    const current = () =>
      within(nav)
        .getAllByRole("link")
        .filter((link) => link.getAttribute("aria-current") === "page")
        .map((link) => link.textContent);
    expect(current()).toEqual(["Transcription"]);
    await userEvent.click(within(nav).getByRole("link", { name: "Polish" }));
    await waitFor(() => expect(app.path()).toBe("/hub/polish"));
    expect(current()).toEqual(["Polish"]);
    await userEvent.click(within(nav).getByRole("link", { name: "Installed" }));
    await waitFor(() => expect(app.path()).toBe("/hub/installed"));
    expect(current()).toEqual(["Installed"]);
  });

  it("the Downloads badge counts pulls and speech model downloads", async () => {
    const { backend } = renderHub("/hub/polish", ({ backend, hub }) => {
      backend.dictation.downloads.push({
        id: PARAKEET,
        name: "Parakeet TDT 0.6B v3",
        state: "verifying",
        completedBytes: 1,
        totalBytes: 1,
        error: null,
      });
      hub.downloads.push({
        model: "gemma3:4b",
        repo: null,
        label: null,
        state: "queued",
        completed: 0,
        total: 0,
        bytesPerSecond: null,
        error: null,
        startedAt: 1,
        finishedAt: null,
      });
    });
    await waitFor(() =>
      expect(screen.getByTestId("active-downloads").textContent).toBe("2"),
    );
    await backend.dictation.finish(PARAKEET);
    await waitFor(() =>
      expect(screen.getByTestId("active-downloads").textContent).toBe("1"),
    );
  });
});

describe("focus kept in the cards (review 43, F10)", () => {
  it("Use, Download, Cancel and the end of a download leave the focus on the card title", async () => {
    const { backend } = renderHub("/hub/transcription", ({ backend }) => {
      backend.dictation.install(PARAKEET);
    });
    await screen.findAllByTestId("speech-model");
    const parakeet = card("Parakeet TDT 0.6B v3");
    const parakeetTitle = within(parakeet).getByRole("heading", {
      name: "Parakeet TDT 0.6B v3",
    });
    await userEvent.click(
      within(parakeet).getByRole("button", { name: "Use" }),
    );
    await within(parakeet).findByText("In use");
    await waitFor(() => expect(document.activeElement).toBe(parakeetTitle));

    const tiny = card("Whisper Tiny");
    const tinyTitle = within(tiny).getByRole("heading", {
      name: "Whisper Tiny",
    });
    await userEvent.click(
      within(tiny).getByRole("button", { name: "Download" }),
    );
    await within(tiny).findByText("Waiting…");
    // Cancel est un autre bouton (pas Download relabellisé) : titre.
    await waitFor(() => expect(document.activeElement).toBe(tinyTitle));
    // La fin du téléchargement retire Cancel, focalisé.
    await backend.dictation.progress(WHISPER_TINY, MIB);
    act(() =>
      within(tiny)
        .getByRole<HTMLButtonElement>("button", { name: "Cancel" })
        .focus(),
    );
    await backend.dictation.finish(WHISPER_TINY);
    await within(tiny).findByRole("button", { name: "Use" });
    await waitFor(() => expect(document.activeElement).toBe(tinyTitle));
  });

  it("Use for polish leaves the focus on the card title", async () => {
    renderHub("/hub/polish", () => {}, { models: ["gemma3:4b"] });
    await screen.findAllByTestId("polish-model");
    const gemma = screen.getByRole("listitem", { name: "gemma3:4b" });
    await userEvent.click(
      await within(gemma).findByRole("button", { name: "Use for polish" }),
    );
    await within(gemma).findByText("In use");
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(gemma).getByRole("heading", { name: "gemma3:4b" }),
      ),
    );
  });

  it("focus elsewhere in the page is left alone", async () => {
    renderHub("/hub/polish", () => {}, { models: ["gemma3:4b"] });
    await screen.findAllByTestId("polish-model");
    const gemma = screen.getByRole("listitem", { name: "gemma3:4b" });
    const use = await within(gemma).findByRole("button", {
      name: "Use for polish",
    });
    act(() => use.focus());
    const tab = screen.getByRole("link", { name: "Downloads" });
    act(() => tab.focus());
    act(() => use.click());
    await within(gemma).findByText("In use");
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(document.activeElement).toBe(tab);
  });
});

describe("Polish tab without Ollama (review 43, F2)", () => {
  it("unreachable: said so, neither Download nor Use", async () => {
    const { simulator } = renderHub("/hub/polish");
    simulator.handle("list_models", () => {
      throw { kind: "ollama", message: "connection refused" };
    });
    await screen.findByText(
      "Ollama is unreachable. Start it to download or use these models.",
    );
    for (const element of screen.getAllByTestId("polish-model")) {
      expect(within(element).queryAllByRole("button")).toEqual([]);
    }
  });

  it("not answered yet: neither Download nor Use, nor unreachable", async () => {
    const { simulator } = renderHub("/hub/polish");
    simulator.handle("list_models", () => new Promise(() => {}));
    await screen.findAllByTestId("polish-model");
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    for (const element of screen.getAllByTestId("polish-model")) {
      expect(within(element).queryAllByRole("button")).toEqual([]);
    }
    expect(screen.queryByText(/Ollama is unreachable/)).toBeNull();
  });
});
