import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { Profiler } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ServerLostBanner } from "@/components/ServerLostBanner";
import { healthKey, useServerHealth } from "./useServerHealth";

afterEach(() => {
  cleanup();
  clearMocks();
});

/** Les abonnés de react-query sont prévenus après un setTimeout(0). */
const settle = () =>
  act(() => new Promise((resolve) => setTimeout(resolve, 20)));

function Probe() {
  const { healthy } = useServerHealth();
  return <span data-testid="healthy">{String(healthy)}</span>;
}

describe("useServerHealth", () => {
  it("a check that changes nothing does not render again", async () => {
    let checks = 0;
    let state = "running";
    mockIPC((command) => {
      if (command !== "server_status") throw new Error(command);
      checks++;
      return { url: "http://127.0.0.1:11434", state };
    });
    const client = new QueryClient({
      defaultOptions: { queries: { networkMode: "always", retry: false } },
    });
    const onRender = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <Profiler id="probe" onRender={onRender}>
          <Probe />
        </Profiler>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(client.getQueryData(healthKey)).toBeDefined());
    await settle();
    const renders = onRender.mock.calls.length;

    await act(() => client.refetchQueries({ queryKey: healthKey }));
    await settle();
    await act(() => client.refetchQueries({ queryKey: healthKey }));
    await settle();
    expect(checks).toBe(3);
    expect(onRender).toHaveBeenCalledTimes(renders);

    // Ollama perdu : là, si.
    state = "connectionFailed";
    await act(() => client.refetchQueries({ queryKey: healthKey }));
    await settle();
    expect(screen.getByTestId("healthy").textContent).toBe("false");
  });

  it("says the built-in Ollama is starting instead of not responding", async () => {
    let state = "builtinStarting";
    mockIPC((command) => {
      if (command !== "server_status") throw new Error(command);
      return { url: "http://127.0.0.1:41234", state };
    });
    const client = new QueryClient({
      defaultOptions: { queries: { networkMode: "always", retry: false } },
    });
    function Banner() {
      const health = useServerHealth();
      return health.healthy ? null : (
        <ServerLostBanner onRetry={health.retry} starting={health.starting} />
      );
    }
    render(
      <QueryClientProvider client={client}>
        <Banner />
      </QueryClientProvider>,
    );
    expect((await screen.findByRole("status")).textContent).toBe(
      "Starting Ollama…",
    );
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(screen.queryByText("Ollama isn't responding.")).toBeNull();

    state = "notResponding";
    await act(() => client.refetchQueries({ queryKey: healthKey }));
    await settle();
    expect(screen.getByRole("alert").textContent).toContain(
      "Ollama isn't responding.",
    );
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("restarts the built-in Ollama that failed during the session, with its log", async () => {
    let state = "builtinFailed";
    let restartError: { kind: string; message: string } | null = {
      kind: "system",
      message: "no binary",
    };
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      switch (command) {
        case "server_status":
          return { url: "http://127.0.0.1:41234", state };
        case "restart_builtin_ollama":
          if (restartError) throw restartError;
          state = "builtinStarting";
          return null;
        case "open_builtin_log":
          throw { kind: "system", message: "no viewer" };
        default:
          throw new Error(command);
      }
    });
    const client = new QueryClient({
      defaultOptions: { queries: { networkMode: "always", retry: false } },
    });
    function Banner() {
      const health = useServerHealth();
      return health.healthy ? null : (
        <ServerLostBanner
          onRetry={health.retry}
          starting={health.starting}
          failed={health.failed}
        />
      );
    }
    render(
      <QueryClientProvider client={client}>
        <Banner />
      </QueryClientProvider>,
    );
    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("span")?.textContent).toBe(
      "The built-in Ollama failed to start.",
    );
    // Son journal, et l'échec de son ouverture.
    await userEvent.click(screen.getByRole("button", { name: "Open log" }));
    expect(calls).toContain("open_builtin_log");
    expect(
      await screen.findByText("Couldn't open the log: no viewer"),
    ).toBeTruthy();
    // Retry le relance ; un échec est dit, puis la relance réussit.
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByText("Couldn't start Ollama: no binary"),
    ).toBeTruthy();
    restartError = null;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect((await screen.findByRole("status")).textContent).toBe(
      "Starting Ollama…",
    );
    expect(calls.filter((c) => c === "restart_builtin_ollama")).toHaveLength(2);
  });

  it("does not restart anything when Ollama of this computer stops answering", async () => {
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      if (command !== "server_status") throw new Error(command);
      return { url: "http://127.0.0.1:11434", state: "notResponding" };
    });
    const client = new QueryClient({
      defaultOptions: { queries: { networkMode: "always", retry: false } },
    });
    function Banner() {
      const health = useServerHealth();
      return health.healthy ? null : (
        <ServerLostBanner onRetry={health.retry} failed={health.failed} />
      );
    }
    render(
      <QueryClientProvider client={client}>
        <Banner />
      </QueryClientProvider>,
    );
    await screen.findByText("Ollama isn't responding.");
    expect(screen.queryByRole("button", { name: "Open log" })).toBeNull();
    const checks = calls.length;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(calls.length).toBeGreaterThan(checks));
    expect(calls.every((c) => c === "server_status")).toBe(true);
  });
});
