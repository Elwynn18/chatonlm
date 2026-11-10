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
import type { AppError } from "@/bindings/AppError";
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

/**
 * `outdated` : modèles périmés pour le faux moteur, et `error` : refus de
 * outdated_models, posés avant le rendu (la vérification part aussitôt).
 */
function setup(
  installed: InstalledVersion[],
  outdated: string[] = [],
  error: AppError | null = null,
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  for (const model of outdated) backend.outdated.add(model);
  backend.outdatedError = error;
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

const A = { model: "unsloth/Qwen3-0.6B-GGUF:Q4_K_M", version: "1" };
const B = { model: "unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M", version: "2" };

describe("useOutdatedModels", () => {
  it("asks once for the installed versions, whatever their order", async () => {
    const { rerender, asked } = setup([A, B], [B.model]);
    await waitFor(() => expect(probe()).toBe(B.model));
    expect(asked()).toEqual([[A.model, B.model]]);
    rerender([B, A]);
    rerender([{ ...A }, { ...B }]);
    expect(probe()).toBe(B.model);
    expect(asked()).toHaveLength(1);
  });

  it("asks nothing for an empty list", async () => {
    const { asked } = setup([]);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(probe()).toBe("none");
    expect(asked()).toEqual([]);
  });

  it("checks again when an installed version changes, keeping the previous answer meanwhile", async () => {
    const { simulator, rerender, asked } = setup([A, B], [A.model, B.model]);
    await waitFor(() => expect(probe()).toBe(`${A.model},${B.model}`));

    // A est téléchargé de nouveau (autre version) : la réponse suivante se
    // fait attendre.
    let answer: (names: string[]) => void = () => {};
    simulator.handle(
      "outdated_models",
      () => new Promise<string[]>((resolve) => (answer = resolve)),
    );
    rerender([{ ...A, version: "3" }, B]);
    await waitFor(() => expect(asked()).toHaveLength(2));
    expect(probe()).toBe(`${A.model},${B.model}`);
    answer([B.model]);
    await waitFor(() => expect(probe()).toBe(B.model));
  });

  it("a failed check says nothing", async () => {
    const { asked } = setup([A], [A.model], {
      kind: "internal",
      message: "unreadable",
    });
    await waitFor(() => expect(asked()).toHaveLength(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(probe()).toBe("none");
    // Pas de nouvel essai en boucle.
    expect(asked()).toHaveLength(1);
  });

  it("compares ids like the engine: the quantization without case", () => {
    expect(listsModel([A.model], A.model.replace("Q4_K_M", "q4_k_m"))).toBe(
      true,
    );
    expect(listsModel([A.model], A.model.toLowerCase())).toBe(false);
    expect(listsModel([A.model, B.model], "a/Other-GGUF:Q8_0")).toBe(false);
    expect(listsModel([], A.model)).toBe(false);
  });

  it("forgets a model in the answers kept, the others staying as they are", async () => {
    const { queryClient } = setup([]);
    const first = ["outdatedModels", "x", "1"];
    const second = ["outdatedModels", "x", "2"];
    const other = [A.model];
    queryClient.setQueryData(first, [B.model, A.model]);
    queryClient.setQueryData(second, other);
    forgetOutdated(queryClient, B.model.replace("Q4_K_M", "q4_k_m"));
    expect(queryClient.getQueryData(first)).toEqual([A.model]);
    expect(queryClient.getQueryData(second)).toBe(other);
  });
});
