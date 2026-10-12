import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AccountStatus } from "@/bindings/AccountStatus";
import type { CloudStatus } from "@/bindings/CloudStatus";
import {
  accountKey,
  accountQueryOptions,
  SIGN_IN_POLL_MS,
  useAccount,
  useSignIn,
} from "./useAccount";
import { cloudStatusKey, useCloudStatus } from "./useCloudStatus";

afterEach(() => {
  cleanup();
  clearMocks();
  vi.useRealTimers();
});

const ADA: AccountStatus = {
  state: "signedIn",
  name: "ada",
  email: "ada@example.com",
  plan: "free",
  avatar: null,
};

/**
 * Backend simulé : `answers` répond à chaque commande (lancer = erreur).
 * `retry` : nouveaux essais par défaut du client (ceux de l'app : aucun).
 */
function setup(answers: Record<string, () => unknown>, retry = 0) {
  const calls: string[] = [];
  mockIPC((command) => {
    calls.push(command);
    const answer = answers[command];
    if (!answer) throw new Error(`unexpected command ${command}`);
    return answer();
  });
  const client = new QueryClient({
    defaultOptions: {
      queries: { networkMode: "always", retry, retryDelay: 0 },
    },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { calls, client, wrapper };
}

describe("useAccount", () => {
  it("uses the official query options", () => {
    expect(accountQueryOptions.queryKey).toEqual(["account"]);
    expect(accountQueryOptions.staleTime).toBe(5 * 60 * 1000);
    expect(accountQueryOptions.gcTime).toBe(10 * 60 * 1000);
    expect(accountQueryOptions.retry).toBe(10);
    const delay = accountQueryOptions.retryDelay as (n: number) => number;
    expect([1, 2, 3, 4, 5, 10].map(delay)).toEqual([
      500, 1000, 1500, 2000, 2000, 2000,
    ]);
  });

  it("reads the account, retrying a failed read", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let failures = 1;
    const { calls, wrapper } = setup({
      account_status: () => {
        if (failures-- > 0) throw { kind: "ollama", message: "down" };
        return ADA;
      },
    });
    const { result } = renderHook(() => useAccount(), { wrapper });
    expect(result.current.isLoading).toBe(true);
    await act(() => vi.advanceTimersByTimeAsync(1000));
    await waitFor(() => expect(result.current.isAuthenticated).toBe(true));
    expect(result.current.account).toEqual(ADA);
    expect(result.current.isLoading).toBe(false);
    expect(calls).toEqual(["account_status", "account_status"]);
  });

  it("is not read while disabled", async () => {
    const { calls, wrapper } = setup({ account_status: () => ADA });
    const { result } = renderHook(() => useAccount({ enabled: false }), {
      wrapper,
    });
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(calls).toEqual([]);
    expect(result.current.isLoading).toBe(false);
    expect(result.current.account).toBeUndefined();
  });

  it("refresh reads the account again and updates the cache", async () => {
    let account: AccountStatus = { state: "signedOut" };
    const { client, wrapper } = setup({ account_status: () => account });
    const { result } = renderHook(() => useAccount(), { wrapper });
    await waitFor(() =>
      expect(result.current.account).toEqual({ state: "signedOut" }),
    );
    account = ADA;
    let read: AccountStatus | undefined;
    await act(async () => {
      read = await result.current.refresh();
    });
    expect(read).toEqual(ADA);
    expect(client.getQueryData(accountKey)).toEqual(ADA);
    await waitFor(() => expect(result.current.isAuthenticated).toBe(true));
  });

  it("signOut shows the account signed out at once, then reads it again", async () => {
    let account: AccountStatus = ADA;
    let release!: () => void;
    const { calls, client, wrapper } = setup({
      account_status: () => account,
      account_sign_out: () =>
        new Promise((resolve) => {
          release = () => {
            account = { state: "signedOut" };
            resolve(null);
          };
        }),
    });
    const { result } = renderHook(() => useAccount(), { wrapper });
    await waitFor(() => expect(result.current.isAuthenticated).toBe(true));

    let done!: Promise<unknown>;
    act(() => {
      done = result.current.signOut();
    });
    await waitFor(() =>
      expect(client.getQueryData(accountKey)).toEqual({ state: "signedOut" }),
    );
    expect(result.current.isAuthenticated).toBe(false);
    await act(async () => {
      release();
      await done;
    });
    await waitFor(() =>
      expect(calls.filter((c) => c === "account_status")).toHaveLength(2),
    );
  });

  it("a failed signOut gives the account back before reading it again", async () => {
    // Relecture après l'échec retenue : seul le compte rétabli est montré.
    let reads = 0;
    let answer!: () => void;
    const { client, wrapper } = setup({
      account_status: () => {
        reads += 1;
        if (reads === 1) return ADA;
        return new Promise((resolve) => (answer = () => resolve(ADA)));
      },
      account_sign_out: () => {
        throw { kind: "ollama", message: "status 500" };
      },
    });
    const { result } = renderHook(() => useAccount(), { wrapper });
    await waitFor(() => expect(result.current.isAuthenticated).toBe(true));
    await act(async () => {
      await expect(result.current.signOut()).rejects.toEqual({
        kind: "ollama",
        message: "status 500",
      });
    });
    expect(client.getQueryData(accountKey)).toEqual(ADA);
    await waitFor(() => expect(result.current.isAuthenticated).toBe(true));
    await waitFor(() => expect(reads).toBe(2));
    await act(async () => answer());
  });

  it("useSignIn stops waiting as soon as the account is signed in", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { calls, wrapper } = setup({ account_sign_in: () => null });
    const refresh = vi.fn(() =>
      Promise.resolve<AccountStatus>({ state: "signedOut" }),
    );
    const { result, rerender } = renderHook(
      ({ isAuthenticated }) => useSignIn({ isAuthenticated, refresh }),
      { wrapper, initialProps: { isAuthenticated: false } },
    );
    await act(() => result.current.start());
    expect(calls).toEqual(["account_sign_in"]);
    expect(result.current.awaiting).toBe(true);
    // Une relecture tout de suite, puis toutes les 5 s.
    expect(refresh).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(SIGN_IN_POLL_MS));
    expect(refresh).toHaveBeenCalledTimes(2);

    // Relié par une autre relecture (bandeau, retour sur la fenêtre...).
    rerender({ isAuthenticated: true });
    expect(result.current.awaiting).toBe(false);
    await act(() => vi.advanceTimersByTimeAsync(3 * SIGN_IN_POLL_MS));
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("useSignIn: a server already signed in ends the waiting at once", async () => {
    const { wrapper } = setup({ account_sign_in: () => null });
    const refresh = vi.fn(() => Promise.resolve<AccountStatus>(ADA));
    const { result } = renderHook(
      () => useSignIn({ isAuthenticated: false, refresh }),
      { wrapper },
    );
    await act(() => result.current.start());
    expect(refresh).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(result.current.awaiting).toBe(false));
  });

  it("useSignIn: one page at a time while it is being opened", async () => {
    let open!: () => void;
    const { calls, wrapper } = setup({
      account_sign_in: () =>
        new Promise((resolve) => (open = () => resolve(null))),
    });
    const refresh = vi.fn(() =>
      Promise.resolve<AccountStatus>({ state: "signedOut" }),
    );
    const { result } = renderHook(
      () => useSignIn({ isAuthenticated: false, refresh }),
      { wrapper },
    );
    let first!: Promise<void>;
    act(() => {
      first = result.current.start();
      void result.current.start();
    });
    expect(result.current.starting).toBe(true);
    await waitFor(() => expect(calls).toEqual(["account_sign_in"]));
    await act(async () => {
      open();
      await first;
    });
    expect(result.current.starting).toBe(false);
    expect(result.current.awaiting).toBe(true);
    expect(calls).toEqual(["account_sign_in"]);
  });

  it("ignores a read started before a sign-out", async () => {
    // Lectures retenues, dans l'ordre : chacune répond quand le test le dit.
    let hold = false;
    const pending: ((account: AccountStatus) => void)[] = [];
    const { client, wrapper } = setup({
      account_status: () =>
        hold ? new Promise((resolve) => pending.push(resolve)) : ADA,
      account_sign_out: () => null,
    });
    const { result } = renderHook(() => useAccount(), { wrapper });
    await waitFor(() => expect(result.current.isAuthenticated).toBe(true));

    hold = true;
    let read!: Promise<AccountStatus | undefined>;
    act(() => {
      read = result.current.refresh();
    });
    await waitFor(() => expect(pending).toHaveLength(1));
    await act(() => result.current.signOut());
    expect(client.getQueryData(accountKey)).toEqual({ state: "signedOut" });
    // La relecture partie avant la déconnexion répond « relié » : ignorée.
    await act(async () => {
      pending[0](ADA);
      expect(await read).toBeUndefined();
    });
    expect(client.getQueryData(accountKey)).toEqual({ state: "signedOut" });
    // Celle d'après la déconnexion compte.
    await waitFor(() => expect(pending).toHaveLength(2));
    await act(async () => pending[1]({ state: "signedOut" }));
    expect(result.current.isAuthenticated).toBe(false);
  });
});

describe("useCloudStatus", () => {
  it.each([
    [{ disabled: true, source: "env" }, true, true],
    [{ disabled: false, source: "none" }, false, true],
    [null, false, false],
  ] as [CloudStatus | null, boolean, boolean][])(
    "%j: cloudDisabled %s, known %s",
    async (status, disabled, known) => {
      const { client, wrapper } = setup({ cloud_status: () => status });
      const { result } = renderHook(() => useCloudStatus(), { wrapper });
      await waitFor(() =>
        expect(client.getQueryState(cloudStatusKey)?.status).toBe("success"),
      );
      expect(result.current.cloudDisabled).toBe(disabled);
      expect(result.current.isKnown).toBe(known);
    },
  );

  it("a failed read counts as not disabled, without retry", async () => {
    const { calls, client, wrapper } = setup(
      {
        cloud_status: () => {
          throw { kind: "ollama", message: "down" };
        },
      },
      3,
    );
    const { result } = renderHook(() => useCloudStatus(), { wrapper });
    await waitFor(() =>
      expect(client.getQueryState(cloudStatusKey)?.status).toBe("error"),
    );
    expect(result.current.cloudDisabled).toBe(false);
    expect(calls).toEqual(["cloud_status"]);
  });

  it("is not read while disabled", async () => {
    const { calls, wrapper } = setup({ cloud_status: () => null });
    renderHook(() => useCloudStatus({ enabled: false }), { wrapper });
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(calls).toEqual([]);
  });
});
