// Liste des téléchargements : lecture, événements du moteur, réponse en
// retard sur un événement, relecture des installés à la fin ; un seul
// écouteur, monté avec l'app (pas seulement avec le ModelHub).

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  configure,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { HubDownload } from "@/bindings/HubDownload";
import { hubInstalledKey } from "@/hooks/useHub";
import { modelsKey } from "@/hooks/useModels";
import { outdatedKey } from "@/hooks/useOutdatedModels";
import { fakeBackend } from "@/test/backend";
import { renderHub } from "@/test/renderHub";
import { installSimulator, type Simulator } from "@/test/simulator";
import { useDownloadEvents, useDownloads } from "./useDownloads";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(cleanup);

const MODEL = "hf.co/a/b-GGUF:Q4_K_M";

function download(fields: Partial<HubDownload>): HubDownload {
  return {
    model: MODEL,
    repo: "a/b-GGUF",
    label: "Q4_K_M",
    state: "downloading",
    completed: 0,
    total: 100,
    bytesPerSecond: null,
    error: null,
    startedAt: 1,
    finishedAt: null,
    ...fields,
  };
}

/** Écouteur de l'app (route racine). */
function Events() {
  useDownloadEvents();
  return null;
}

function Probe() {
  const { downloads, loaded } = useDownloads();
  return (
    <p data-testid="probe">
      {loaded
        ? downloads.map((d) => `${d.state}:${d.completed}`).join(",") || "none"
        : "loading"}
    </p>
  );
}

/** `prepare` : gestionnaires posés avant le premier rendu (première lecture). */
function setup(prepare?: (simulator: Simulator) => void) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare?.(simulator);
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Events />
      <Probe />
    </QueryClientProvider>,
  );
  return { simulator, hub: backend.hub, client };
}

const probe = () => screen.getByTestId("probe").textContent;

describe("useDownloads", () => {
  it("reads the list, then follows the events", async () => {
    const { simulator } = setup();
    await waitFor(() => expect(probe()).toBe("none"));
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
    await simulator.emitEvent("hub-downloads", [download({ completed: 40 })]);
    await waitFor(() => expect(probe()).toBe("downloading:40"));
  });

  it("an answer older than an event does not overwrite it", async () => {
    let answer: ((list: HubDownload[]) => void) | null = null;
    const { simulator } = setup((simulator) =>
      simulator.handle(
        "hub_downloads",
        () => new Promise((resolve) => (answer = resolve)),
      ),
    );
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
    await waitFor(() => expect(answer).not.toBeNull());
    expect(probe()).toBe("loading");
    // L'événement arrive avant la réponse, qui décrit un état plus ancien.
    await simulator.emitEvent("hub-downloads", [
      download({ state: "done", completed: 100 }),
    ]);
    answer!([download({ completed: 10 })]);
    await waitFor(() => expect(probe()).toBe("done:100"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(probe()).toBe("done:100");
  });

  it("a finished download makes the picker and the installed list read again, once", async () => {
    const { simulator, client } = setup();
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
    client.setQueryData(modelsKey, []);
    client.setQueryData([...hubInstalledKey, "x"], []);
    await simulator.emitEvent("hub-downloads", [download({ completed: 50 })]);
    await waitFor(() => expect(probe()).toBe("downloading:50"));
    expect(client.getQueryState(modelsKey)?.isInvalidated).toBe(false);

    const done = download({ state: "done", completed: 100, finishedAt: 9 });
    await simulator.emitEvent("hub-downloads", [done]);
    await waitFor(() => expect(probe()).toBe("done:100"));
    expect(client.getQueryState(modelsKey)?.isInvalidated).toBe(true);
    expect(client.getQueryState([...hubInstalledKey, "x"])?.isInvalidated).toBe(
      true,
    );

    // Le même état reçu de nouveau : rien à relire.
    client.setQueryData(modelsKey, []);
    await simulator.emitEvent("hub-downloads", [done]);
    expect(client.getQueryState(modelsKey)?.isInvalidated).toBe(false);
  });

  it("a model downloaded again is no longer stale, at once and in every check kept", async () => {
    const { simulator, client } = setup();
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
    const other = "qwen3:0.6b";
    // Vérification de la conversation (nom du sélecteur) et de la liste des
    // installés (nom de /api/tags).
    const chat = [...outdatedKey, "x", "chat"];
    const list = [...outdatedKey, "x", "installed"];
    const untouched = [other];
    client.setQueryData(chat, ["gemma4"]);
    client.setQueryData(list, [other, "gemma4:latest"]);
    client.setQueryData([...outdatedKey, "x", "other"], untouched);
    const gemma = { model: "Gemma4:latest", repo: null, label: null };

    // En cours, en échec, arrêté : toujours périmé.
    for (const state of ["downloading", "failed", "cancelled"] as const) {
      await simulator.emitEvent("hub-downloads", [
        download({ ...gemma, state }),
      ]);
      await waitFor(() => expect(probe()).toBe(`${state}:0`));
      expect(client.getQueryData(chat)).toEqual(["gemma4"]);
    }

    await simulator.emitEvent("hub-downloads", [
      download({ ...gemma, state: "done", completed: 100, finishedAt: 9 }),
    ]);
    await waitFor(() => expect(probe()).toBe("done:100"));
    expect(client.getQueryData(chat)).toEqual([]);
    expect(client.getQueryData(list)).toEqual([other]);
    // Une vérification qui ne le citait pas garde sa réponse telle quelle.
    expect(client.getQueryData([...outdatedKey, "x", "other"])).toBe(untouched);
  });

  it("stops listening when unmounted", async () => {
    const { simulator } = setup();
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
    cleanup();
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(0));
  });
});

describe("downloads in the app", () => {
  it("a download finishing while the chat is shown, sidebar closed, reads the models again", async () => {
    const { simulator, app } = renderHub("/c/new");
    await screen.findByLabelText("Message");
    // Barre latérale fermée : ni ModelHub ni indicateur montés.
    expect(screen.queryByRole("link", { name: "ModelHub" })).toBeNull();
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
    await waitFor(() =>
      expect(simulator.callsTo("list_models").length).toBeGreaterThan(0),
    );
    const before = simulator.callsTo("list_models").length;
    await simulator.emitEvent("hub-downloads", [
      download({ state: "done", completed: 100, finishedAt: 9 }),
    ]);
    await waitFor(() =>
      expect(simulator.callsTo("list_models").length).toBeGreaterThan(before),
    );
    expect(app.queryClient.getQueryData(["hub", "downloads"])).toHaveLength(1);
  });

  it("listens once, however many views show the downloads", async () => {
    const { simulator } = renderHub("/hub/downloads");
    await screen.findByText("No downloads yet.");
    await screen.findByRole("link", { name: "Downloads" });
    await waitFor(() => expect(simulator.listening("hub-downloads")).toBe(1));
  });
});
