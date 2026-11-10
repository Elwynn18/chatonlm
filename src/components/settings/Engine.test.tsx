// Ligne « Engine » de Settings › General face au faux moteur : version de
// llama.cpp, accélération (carte nommée, carte sans nom, processeur, CUDA),
// repli de la session, moteur absent, « Open logs » et son échec, langues ;
// section « NVIDIA acceleration » (pack CUDA) : cachée sans carte NVIDIA,
// raisons d'incompatibilité, licence à accepter avant le téléchargement,
// progression et annulation, état suivi par l'événement, interrupteur,
// retrait, repli de la session, erreurs.

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
import type { CudaStatus } from "@/bindings/CudaStatus";
import type { EngineStatus } from "@/bindings/EngineStatus";
import { setLanguage } from "@/i18n";
import { createQueryClient } from "@/lib/queryClient";
import { ENGINE_GPU, fakeEngine, noCuda } from "@/test/engine";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { installSimulator } from "@/test/simulator";
import { EngineRow } from "./Engine";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

/**
 * La ligne seule ; `status` retouche l'état du faux moteur, `cuda` celui de
 * son pack CUDA.
 */
function renderRow(
  status: Partial<EngineStatus> = {},
  cuda: Partial<CudaStatus> = {},
) {
  const simulator = installSimulator();
  const engine = fakeEngine(simulator);
  Object.assign(engine.status, status);
  Object.assign(engine.cuda, cuda);
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

  it("names the NVIDIA card when CUDA runs the models", async () => {
    renderRow({ backend: "cuda" });
    await waitFor(() =>
      expect(screen.getByTestId("engine-acceleration").textContent).toBe(
        `Acceleration: ${ENGINE_GPU} (CUDA)`,
      ),
    );
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

const MIB = 1024 ** 2;

/** Pack CUDA compatible, pas installé. */
const COMPATIBLE: Partial<CudaStatus> = {
  compat: "compatible",
  gpuName: ENGINE_GPU,
  driver: "615.78.08",
  compute: "8.6",
};

describe("NVIDIA acceleration", () => {
  it("is hidden without an NVIDIA card", async () => {
    const { simulator } = renderRow();
    await waitFor(() =>
      expect(simulator.callsTo("cuda_status")).toEqual([{ fresh: true }]),
    );
    expect(screen.queryByTestId("engine-cuda")).toBeNull();
  });

  it("asks for NVIDIA's license before downloading", async () => {
    const { simulator } = renderRow({}, COMPATIBLE);
    simulator.handle("open_external", () => null);
    expect((await screen.findByTestId("engine-cuda-offer")).textContent).toBe(
      `Runs models faster on ${ENGINE_GPU} with CUDA. Optional download of about 1.1 GB: ChatonLM's CUDA engine, and NVIDIA's libraries from NVIDIA's servers.`,
    );
    expect(screen.getByRole("heading").textContent).toBe(
      "NVIDIA acceleration (CUDA)",
    );
    await userEvent.click(screen.getByRole("button", { name: "Download…" }));
    const confirm = screen.getByRole("button", { name: "Download" });
    expect((confirm as HTMLButtonElement).disabled).toBe(true);
    await userEvent.click(
      screen.getByRole("button", { name: "Read NVIDIA's license" }),
    );
    expect(simulator.callsTo("open_external")).toEqual([
      { url: "https://docs.nvidia.com/cuda/eula/index.html" },
    ]);
    expect(simulator.callsTo("cuda_install")).toEqual([]);
    await userEvent.click(
      screen.getByRole("checkbox", { name: "I accept NVIDIA's license" }),
    );
    await userEvent.click(confirm);
    await waitFor(() =>
      expect(simulator.callsTo("cuda_install")).toEqual([
        { acceptLicense: true },
      ]),
    );
    // Retour à l'offre, sans la licence (l'état suivant vient de l'événement).
    await screen.findByTestId("engine-cuda-offer");
  });

  it("can step back from the license", async () => {
    const { simulator } = renderRow({}, COMPATIBLE);
    await userEvent.click(
      await screen.findByRole("button", { name: "Download…" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByTestId("engine-cuda-license")).toBeNull();
    expect(simulator.callsTo("cuda_install")).toEqual([]);
  });

  it("follows the download, cancels it, then shows the installed pack", async () => {
    const { simulator } = renderRow({}, COMPATIBLE);
    await screen.findByTestId("engine-cuda-offer");
    await simulator.emitEvent("engine-cuda", {
      ...noCuda(),
      ...COMPATIBLE,
      phase: "downloading",
      completedBytes: 256 * MIB,
      totalBytes: 1024 * MIB,
    } satisfies CudaStatus);
    const bar = await screen.findByRole("progressbar", {
      name: "Download of the NVIDIA pack",
    });
    expect(bar.getAttribute("aria-valuenow")).toBe("25");
    expect(screen.getByTestId("engine-cuda-progress").textContent).toContain(
      "Downloading: 256 MB of 1.0 GB (25%)",
    );
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(simulator.callsTo("cuda_cancel")).toHaveLength(1),
    );

    await simulator.emitEvent("engine-cuda", {
      ...noCuda(),
      ...COMPATIBLE,
      phase: "installed",
      installedBytes: 800 * MIB,
      active: true,
    } satisfies CudaStatus);
    expect((await screen.findByTestId("engine-cuda-state")).textContent).toBe(
      `Models load on ${ENGINE_GPU} with CUDA. Installed (800 MB).`,
    );
    const toggle = screen.getByRole("switch", { name: "Use CUDA" });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(toggle);
    await waitFor(() =>
      expect(simulator.callsTo("cuda_set_enabled")).toEqual([
        { enabled: false },
      ]),
    );
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(simulator.callsTo("cuda_remove")).toHaveLength(1),
    );
  });

  it("says why an old driver or an old card can't use CUDA", async () => {
    renderRow({}, { ...COMPATIBLE, compat: "driverTooOld", driver: "575.64" });
    expect(
      (await screen.findByTestId("engine-cuda-incompatible")).textContent,
    ).toBe(
      "The NVIDIA driver 575.64 is too old for CUDA 13: version 580 or later is needed. Until then, models use Vulkan.",
    );
    expect(screen.queryByRole("button", { name: "Download…" })).toBeNull();
    cleanup();
    renderRow(
      {},
      {
        ...COMPATIBLE,
        compat: "computeTooLow",
        gpuName: "GeForce GTX 1080",
        compute: "6.1",
      },
    );
    expect(
      (await screen.findByTestId("engine-cuda-incompatible")).textContent,
    ).toBe(
      "GeForce GTX 1080 is too old for CUDA 13 (compute capability 6.1, 7.5 needed): models use Vulkan, which works well on it.",
    );
  });

  it("shows the session fallback, a stale pack and the errors", async () => {
    const { engine } = renderRow(
      {},
      {
        ...COMPATIBLE,
        stale: true,
        fallback: "CUDA error: out of memory",
        error: "the download of the NVIDIA pack failed: timeout",
      },
    );
    expect((await screen.findByTestId("engine-cuda-offer")).textContent).toBe(
      "The NVIDIA pack on this computer is for another version of the engine. Download it again to use CUDA.",
    );
    expect(screen.getByTestId("engine-cuda-fallback").textContent).toBe(
      "CUDA failed during this session (CUDA error: out of memory): models use Vulkan until ChatonLM restarts or CUDA is turned back on.",
    );
    expect(screen.getByRole("alert").textContent).toBe(
      "the download of the NVIDIA pack failed: timeout",
    );
    // Commande refusée : dite sous la section.
    engine.failures.set("cuda_install", {
      kind: "invalid",
      message: "this computer can't use the NVIDIA pack",
    });
    await userEvent.click(screen.getByRole("button", { name: "Download…" }));
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    expect((await screen.findByText(/couldn't be changed/)).textContent).toBe(
      "The NVIDIA pack couldn't be changed: this computer can't use the NVIDIA pack",
    );
  });

  it("says when its state can't be read", async () => {
    const simulator = installSimulator();
    const engine = fakeEngine(simulator);
    engine.failures.set("cuda_status", { kind: "internal", message: "boom" });
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
      "Could not read the state of the NVIDIA pack: boom",
    );
  });

  it("has every text translated", async () => {
    usePseudoLanguage();
    const { container, simulator } = renderRow(
      {},
      { ...COMPATIBLE, fallback: "CUDA error" },
    );
    await userEvent.click(await screen.findByTestId("engine-cuda-download"));
    await screen.findByTestId("engine-cuda-license");
    expect(
      untranslatedTexts(container, ["v0.9.0", ENGINE_GPU, "CUDA error"]),
    ).toEqual([]);
    await simulator.emitEvent("engine-cuda", {
      ...noCuda(),
      ...COMPATIBLE,
      phase: "installed",
      installedBytes: 1,
    } satisfies CudaStatus);
    await screen.findByTestId("engine-cuda-state");
    expect(untranslatedTexts(container, ["v0.9.0", ENGINE_GPU])).toEqual([]);
  });
});
