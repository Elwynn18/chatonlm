// Vérification des modèles périmés : ce qui est demandé au moteur, quand, et
// ce qui reste affiché pendant qu'une liste changée est vérifiée.

import { QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  configure,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { fakeBackend } from "@/test/backend";
import { installSimulator } from "@/test/simulator";
import {
  forgetOutdated,
  type InstalledVersion,
  listsModel,
  useOutdatedModels,
} from "./useOutdatedModels";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function Probe({ installed }: { installed: InstalledVersion[] }) {
  const outdated = useOutdatedModels(installed);
  return <p data-testid="probe">{outdated.join(",") || "none"}</p>;
}

const probe = () => screen.getByTestId("probe").textContent;

function setup(installed: InstalledVersion[]) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  const queryClient = createQueryClient();
  const show = (list: InstalledVersion[]) => (
    <QueryClientProvider client={queryClient}>
      <Probe installed={list} />
    </QueryClientProvider>
  );
  const view = render(show(installed));
  onTestFinished(() => {
    cleanup();
    queryClient.clear();
  });
  return {
    simulator,
    backend,
    queryClient,
    rerender: (list: InstalledVersion[]) => view.rerender(show(list)),
    asked: () =>
      simulator.callsTo("outdated_models").map((call) => call.models),
  };
}

const A = { model: "qwen3:0.6b", version: "a1" };
const B = { model: "qwen3.5:0.8b", version: "b1" };

describe("useOutdatedModels", () => {
  it("asks once for the installed versions, whatever their order", async () => {
    const { backend, rerender, asked } = setup([A, B]);
    backend.outdated.add(B.model);
    await waitFor(() => expect(probe()).toBe(B.model));
    expect(asked()).toEqual([[A.model, B.model]]);
    rerender([B, A]);
    rerender([{ ...A }, { ...B }]);
    expect(probe()).toBe(B.model);
    expect(asked()).toHaveLength(1);
  });

  it("asks nothing for an empty list", async () => {
    const { simulator, asked } = setup([]);
    await waitFor(() =>
      expect(simulator.callsTo("get_settings").length).toBeGreaterThan(0),
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(probe()).toBe("none");
    expect(asked()).toEqual([]);
  });

  it("checks again when an installed version changes, keeping the previous answer meanwhile", async () => {
    const { simulator, backend, rerender, asked } = setup([A, B]);
    backend.outdated.add(A.model);
    backend.outdated.add(B.model);
    await waitFor(() => expect(probe()).toBe(`${A.model},${B.model}`));

    // A est téléchargé de nouveau (autre version) : la réponse suivante se
    // fait attendre.
    let answer: (names: string[]) => void = () => {};
    simulator.handle(
      "outdated_models",
      () => new Promise<string[]>((resolve) => (answer = resolve)),
    );
    rerender([{ ...A, version: "a2" }, B]);
    await waitFor(() => expect(asked()).toHaveLength(2));
    expect(probe()).toBe(`${A.model},${B.model}`);
    answer([B.model]);
    await waitFor(() => expect(probe()).toBe(B.model));
  });

  it("an unreachable Ollama or registry says nothing", async () => {
    const { backend, asked } = setup([A]);
    backend.outdated.add(A.model);
    backend.outdatedError = { kind: "ollama", message: "unreachable" };
    await waitFor(() => expect(asked()).toHaveLength(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(probe()).toBe("none");
    // Pas de nouvel essai en boucle.
    expect(asked()).toHaveLength(1);
  });

  it("compares names without case, latest being implied", () => {
    expect(listsModel(["gemma4:latest"], "Gemma4")).toBe(true);
    expect(listsModel(["gemma4"], "gemma4:LATEST")).toBe(true);
    expect(listsModel(["gemma4:27b", "qwen3:0.6b"], "gemma4")).toBe(false);
    expect(listsModel([], "gemma4")).toBe(false);
  });

  it("forgets a model in the answers kept, the others staying as they are", async () => {
    const { queryClient } = setup([]);
    const first = ["outdatedModels", "x", "1"];
    const second = ["outdatedModels", "x", "2"];
    const other = [A.model];
    queryClient.setQueryData(first, ["gemma4", A.model]);
    queryClient.setQueryData(second, other);
    forgetOutdated(queryClient, "gemma4:latest");
    expect(queryClient.getQueryData(first)).toEqual([A.model]);
    expect(queryClient.getQueryData(second)).toBe(other);
  });
});
