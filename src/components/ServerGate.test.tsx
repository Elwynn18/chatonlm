import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { ServerState } from "@/bindings/ServerState";
import type { ServerStatus } from "@/bindings/ServerStatus";
import { serverStatusKey } from "@/hooks/useServerStatus";
import { createQueryClient } from "@/lib/queryClient";
import { type FakeBackend, fakeBackend, OLLAMA_LOG } from "@/test/backend";
import { installSimulator } from "@/test/simulator";
import { ServerGate } from "./ServerGate";

afterEach(() => {
  cleanup();
  clearMocks();
});

function status(state: ServerState): ServerStatus {
  return {
    url: "http://localhost:11434",
    state,
    version: state === "running" ? "0.35.1" : null,
    alternativeUrl: null,
    hostEnv: null,
    httpStatus: null,
    detail: null,
    service: null,
    builtin: false,
    builtinAvailable: false,
    explicitAddress: false,
    logPath: null,
  };
}

/** Backend simulé : `server_status` renvoie l'état courant de `answers`. */
function backend(answers: { next: () => Promise<ServerStatus> }) {
  let calls = 0;
  mockIPC((command) => {
    if (command === "server_status") {
      calls += 1;
      return answers.next();
    }
    if (command === "get_settings") return { serverUrl: "" };
    // Écoute des changements de cible (ollama-target).
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return null;
    throw new Error(`unexpected command ${command}`);
  });
  return { calls: () => calls };
}

function renderGate() {
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServerGate>
        <p>the app</p>
      </ServerGate>
    </QueryClientProvider>,
  );
  return client;
}

describe("ServerGate", () => {
  it("shows the logo during the first check, then a message if it lasts", async () => {
    backend({ next: () => new Promise(() => {}) });
    renderGate();
    const message = screen.getByText("Connecting to Ollama…");
    expect(message.className).toContain("invisible");
    await act(() => new Promise((resolve) => setTimeout(resolve, 450)));
    expect(message.className).not.toContain("invisible");
    expect(screen.queryByText("the app")).toBeNull();
  });

  it("explains a failed check and retries it", async () => {
    let fail = true;
    backend({
      next: () =>
        fail
          ? Promise.reject({ kind: "internal", message: "no store" })
          : Promise.resolve(status("running")),
    });
    renderGate();
    expect((await screen.findByRole("alert")).textContent).toBe("no store");
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("the app")).toBeTruthy();
  });

  it("never goes back to the welcome screen once connected", async () => {
    let current = status("running");
    const { calls } = backend({ next: () => Promise.resolve(current) });
    const client = renderGate();
    expect(await screen.findByText("the app")).toBeTruthy();

    // Retour sur la fenêtre : pas de nouvelle vérification.
    await act(async () => {
      window.dispatchEvent(new Event("visibilitychange"));
      window.dispatchEvent(new Event("focus"));
    });
    expect(calls()).toBe(1);

    // Même si une vérification forcée échoue, l'app reste affichée.
    current = status("notResponding");
    await act(() => client.refetchQueries({ queryKey: serverStatusKey }));
    expect(screen.getByText("the app")).toBeTruthy();
  });

  it("says Checking… only after a click on Retry", async () => {
    let release: (value: ServerStatus) => void = () => {};
    let first = true;
    backend({
      next: () => {
        if (first) {
          first = false;
          return Promise.resolve(status("serviceStopped"));
        }
        return new Promise((resolve) => (release = resolve));
      },
    });
    renderGate();
    const retry = await screen.findByRole("button", { name: "Retry" });
    await userEvent.click(retry);
    expect(screen.getByRole("button", { name: "Checking…" })).toBeTruthy();
    await act(async () => release(status("serviceStopped")));
    expect(await screen.findByRole("button", { name: "Retry" })).toBeTruthy();
  });
});

describe("ServerGate and the built-in Ollama", () => {
  vi.setConfig({ testTimeout: 30_000 });

  function setup(prepare: (backend: FakeBackend) => void) {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    prepare(backend);
    const client = createQueryClient();
    render(
      <QueryClientProvider client={client}>
        <ServerGate>
          <p>the app</p>
        </ServerGate>
      </QueryClientProvider>,
    );
    onTestFinished(() => {
      cleanup();
      client.clear();
    });
    return { simulator, backend };
  }

  it("Start Ollama starts the stopped service, then shows the app", async () => {
    const { simulator } = setup((backend) => {
      backend.serverState = "serviceStopped";
      backend.service = { user: false, enabled: true };
      backend.runtime.builtinAvailable = false;
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Start Ollama" }),
    );
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(simulator.callsTo("start_ollama_service")).toHaveLength(1);
    // Ni l'Ollama intégré ni son état : il ne sert pas.
    expect(simulator.callsTo("ollama_runtime")).toHaveLength(0);
  });

  it("shows why the service didn't start", async () => {
    setup((backend) => {
      backend.serverState = "serviceStopped";
      backend.service = { user: false, enabled: true };
      backend.runtimeErrors.start = {
        kind: "system",
        message: "Authentication failed",
      };
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Start Ollama" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't start Ollama: Authentication failed",
    );
  });

  it("switches to the built-in Ollama: Starting Ollama…, then the app", async () => {
    const { simulator, backend } = setup((backend) => {
      backend.serverState = "serviceStopped";
      backend.service = { user: false, enabled: true };
      backend.builtinState = "builtinStarting";
    });
    await userEvent.click(
      await screen.findByRole("button", {
        name: "Use the built-in Ollama instead",
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "Starting Ollama…" }),
    ).toBeTruthy();
    expect(simulator.callsTo("set_ollama_source")).toEqual([
      { source: "builtin" },
    ]);
    // Adresse effective relue (celle de l'intégré).
    expect(simulator.callsTo("get_settings").length).toBeGreaterThan(1);
    expect(screen.queryByText("the app")).toBeNull();
    // Prêt : la vérification suivante (3 s) ouvre l'app.
    backend.serverState = "running";
    expect(
      await screen.findByText("the app", {}, { timeout: 5000 }),
    ).toBeTruthy();
  });

  it("Retry restarts the built-in Ollama that failed, and says when it can't", async () => {
    const { simulator, backend } = setup((backend) => {
      backend.runtime.kind = "builtin";
      backend.serverState = "builtinFailed";
      backend.builtinState = "builtinStarting";
      backend.runtimeErrors.restart = {
        kind: "system",
        message: "ollama binary missing",
      };
    });
    await userEvent.click(await screen.findByRole("button", { name: "Retry" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't start Ollama: ollama binary missing",
    );
    backend.runtimeErrors.restart = null;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("heading", { name: "Starting Ollama…" }),
    ).toBeTruthy();
    expect(simulator.callsTo("restart_builtin_ollama")).toHaveLength(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("Retry doesn't restart anything for Ollama on this computer", async () => {
    const { simulator } = setup((backend) => {
      backend.serverState = "notResponding";
    });
    await userEvent.click(await screen.findByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(simulator.callsTo("server_status").length).toBeGreaterThan(1),
    );
    expect(simulator.callsTo("restart_builtin_ollama")).toHaveLength(0);
  });

  it("reads again what depends on the target when the engine changes it", async () => {
    const { simulator } = setup(() => {});
    expect(await screen.findByText("the app")).toBeTruthy();
    await waitFor(() => expect(simulator.listening("ollama-target")).toBe(1));
    const reads = simulator.callsTo("get_settings").length;
    await simulator.emitEvent("ollama-target", {
      url: "http://127.0.0.1:47811",
      kind: "builtin",
    });
    await waitFor(() =>
      expect(simulator.callsTo("get_settings").length).toBeGreaterThan(reads),
    );
  });

  it("opens the log of the built-in Ollama that failed", async () => {
    const { simulator } = setup((backend) => {
      backend.runtime.kind = "builtin";
      backend.serverState = "builtinFailed";
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Open log" }),
    );
    expect(simulator.callsTo("open_builtin_log")).toHaveLength(1);
  });

  it("goes back to Ollama on this computer when the chosen built-in one failed", async () => {
    const { simulator } = setup((backend) => {
      backend.runtime.source = "builtin";
      backend.runtime.kind = "builtin";
      backend.serverState = "builtinFailed";
    });
    expect(await screen.findByText(OLLAMA_LOG)).toBeTruthy();
    const back = await screen.findByRole("button", {
      name: "Use Ollama on this computer",
    });
    expect(screen.queryByLabelText("Server address")).toBeNull();
    await userEvent.click(back);
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(simulator.callsTo("set_ollama_source")).toEqual([
      { source: "auto" },
    ]);
  });
});
