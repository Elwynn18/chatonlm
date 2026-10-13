import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { accountKey } from "@/hooks/useAccount";
import { cloudStatusKey } from "@/hooks/useCloudStatus";
import { modelsKey, recommendedKey, upstreamKey } from "@/hooks/useModels";
import { healthKey } from "@/hooks/useServerHealth";
import { getLanguage } from "@/i18n";
import { modelDetailsKey, thinkPrefKey } from "@/hooks/useThink";
import { fakeBackend, SERVER_URL } from "@/test/backend";
import { type Args, installSimulator } from "@/test/simulator";
import { settingsKey, useSettings } from "./useSettings";

afterEach(() => {
  cleanup();
});

const SAVED: Settings = {
  serverUrl: "",
  effectiveServerUrl: SERVER_URL,
  contextLength: 0,
  theme: "system",
  selectedModel: "qwen3:0.6b",
  windowWidth: null,
  windowHeight: null,
  webSearchEnabled: false,
  keepAlive: 0,
  preloadModel: false,
  language: "system",
  effectiveLanguage: "en",
  proofreadModel: "",
  proofreadShortcut: false,
  proofreadPaste: true,
  studyModel: "",
  autoRead: false,
};

function newClient() {
  return new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
}

/** useSettings monté sur `client` ; réglages déjà en cache si `cached`. */
function mount(
  client: QueryClient,
  options?: { refresh?: boolean },
  { cached = true }: { cached?: boolean } = {},
) {
  if (cached) client.setQueryData(settingsKey, SAVED);
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useSettings(options), { wrapper });
}

/** Promesse réglée de l'extérieur. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

/** Laisse passer les réponses de l'IPC simulé et les notifications. */
const settle = () =>
  act(() => new Promise((resolve) => setTimeout(resolve, 30)));

describe("useSettings", () => {
  it("the displayed language follows the settings, when read and when saved", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.settings = { ...SAVED, language: "fr", effectiveLanguage: "fr" };
    const client = newClient();

    const { result } = mount(client, undefined, { cached: false });
    await waitFor(() => expect(result.current.settings?.language).toBe("fr"));
    expect(getLanguage()).toBe("fr");
    expect(document.documentElement.lang).toBe("fr");

    await act(() => result.current.setSettings({ language: "es" }));
    expect(getLanguage()).toBe("es");
    // « system » : la langue du bureau, donnée par le moteur (anglais ici).
    await act(() => result.current.setSettings({ language: "system" }));
    expect(getLanguage()).toBe("en");
  });

  it("refresh reads the settings again on mount, even when they are cached", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.settings = { ...SAVED, theme: "dark" };
    const client = newClient();

    const plain = mount(client);
    await settle();
    expect(simulator.callsTo("get_settings")).toHaveLength(0);
    expect(plain.result.current.settings?.theme).toBe("system");

    const refreshing = mount(client, { refresh: true });
    await waitFor(() =>
      expect(refreshing.result.current.settings?.theme).toBe("dark"),
    );
    expect(simulator.callsTo("get_settings")).toHaveLength(1);
    expect(plain.result.current.settings?.theme).toBe("dark");
  });

  it("without refresh, reads the settings once when nothing is cached", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    const client = newClient();

    const { result } = mount(client, undefined, { cached: false });
    await waitFor(() => expect(result.current.settings).toEqual(SAVED));
    mount(client);
    await settle();
    expect(simulator.callsTo("get_settings")).toHaveLength(1);
  });

  it("exposes the error of a failed read, keeping the cached settings", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const broken = { kind: "database" as const, message: "disk I/O error" };
    backend.settingsErrors.get = broken;
    const client = newClient();

    const { result } = mount(client, { refresh: true });

    await waitFor(() => expect(result.current.error).toEqual(broken));
    expect(result.current.settings).toEqual(SAVED);
  });

  it("runs updates one after the other, in order", async () => {
    const simulator = installSimulator();
    const answers: ReturnType<typeof deferred<Settings>>[] = [];
    simulator.handle("update_settings", (args: Args) => {
      const answer = deferred<Settings>();
      answers.push(answer);
      return answer.promise.then((settings) => ({
        ...settings,
        ...(args.patch as SettingsPatch),
      }));
    });
    const client = newClient();
    const { result } = mount(client);

    let first!: Promise<Settings>;
    let second!: Promise<Settings>;
    act(() => {
      first = result.current.setSettings({ theme: "light" });
      second = result.current.setSettings({ theme: "dark" });
    });
    await settle();
    // Le second n'est pas parti tant que le premier n'a pas répondu.
    expect(simulator.callsTo("update_settings")).toEqual([
      { patch: { theme: "light" } },
    ]);

    answers[0].resolve(SAVED);
    await act(() => first);
    await waitFor(() =>
      expect(simulator.callsTo("update_settings")).toHaveLength(2),
    );
    expect(simulator.callsTo("update_settings")[1]).toEqual({
      patch: { theme: "dark" },
    });
    expect(client.getQueryData(settingsKey)).toMatchObject({ theme: "light" });

    answers[1].resolve(SAVED);
    await act(() => second);
    expect(client.getQueryData(settingsKey)).toMatchObject({ theme: "dark" });
  });

  it("an update is not undone by a read that was still running", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const read = deferred<Settings>();
    simulator.handle("get_settings", () => read.promise);
    const client = newClient();
    const { result } = mount(client, { refresh: true });
    await waitFor(() =>
      expect(simulator.callsTo("get_settings")).toHaveLength(1),
    );

    await act(() => result.current.setSettings({ theme: "dark" }));
    expect(backend.settings.theme).toBe("dark");
    // La relecture répond avec les réglages d'avant la mise à jour.
    read.resolve(SAVED);
    await settle();

    expect(client.getQueryData(settingsKey)).toMatchObject({ theme: "dark" });
    expect(result.current.settings?.theme).toBe("dark");
  });

  describe("server-dependent queries", () => {
    const dependent = [
      modelsKey,
      recommendedKey,
      upstreamKey("llama3"),
      modelDetailsKey("qwen3:0.6b"),
      healthKey,
    ];
    // Propres au serveur : oubliés (remis à zéro), pas seulement relus.
    const identity = [accountKey, cloudStatusKey];
    const independent = [
      ["serverStatus"],
      ["chats"],
      thinkPrefKey("qwen3:0.6b"),
    ];

    /** Requêtes en cache, sans observateur (rien n'est relu). */
    function seed(client: QueryClient) {
      for (const key of [...dependent, ...independent, ...identity]) {
        client.setQueryData(key, "data");
      }
    }

    /** Requêtes propres au serveur dont les données ont été oubliées. */
    const forgotten = (client: QueryClient) =>
      identity.filter((key) => client.getQueryData(key) === undefined);

    const invalidated = (client: QueryClient) =>
      [...dependent, ...independent].filter(
        (key) => client.getQueryState(key)?.isInvalidated,
      );

    it("are invalidated when the effective server address changes", async () => {
      const simulator = installSimulator();
      fakeBackend(simulator);
      const client = newClient();
      seed(client);
      const { result } = mount(client);

      await act(() =>
        result.current.setSettings({ serverUrl: "http://10.0.0.2:11434" }),
      );

      expect(invalidated(client)).toEqual(dependent);
      // Une relecture en échec ne garderait pas le compte de l'ancien.
      expect(forgotten(client)).toEqual(identity);
    });

    it("are kept when the effective address stays the same", async () => {
      const simulator = installSimulator();
      fakeBackend(simulator);
      const client = newClient();
      seed(client);
      const { result } = mount(client);

      await act(() => result.current.setSettings({ theme: "dark" }));
      await act(() => result.current.setSettings({ contextLength: 8192 }));
      // Adresse réglée, mais identique à l'adresse automatique.
      await act(() => result.current.setSettings({ serverUrl: SERVER_URL }));

      expect(client.getQueryData(settingsKey)).toMatchObject({
        serverUrl: SERVER_URL,
        theme: "dark",
      });
      expect(invalidated(client)).toEqual([]);
      expect(forgotten(client)).toEqual([]);
    });
  });
});
