import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { AppError } from "@/bindings/AppError";
import type { Think } from "@/bindings/Think";
import { modelsKey } from "@/hooks/useModels";
import { fakeBackend, modelInfo, thinkingModel } from "@/test/backend";
import { installSimulator, type Simulator } from "@/test/simulator";
import { modelDetailsKey, thinkPrefKey, useThink } from "./useThink";

afterEach(() => cleanup());

const QWEN = "qwen3:0.6b";
const GPT = "gpt-oss:20b";
const ON_OFF = thinkingModel(QWEN, { values: [false, true], default: true });
const LEVELS = thinkingModel(GPT, {
  values: ["low", "medium", "high"],
  default: "medium",
});

/**
 * Hook pour `model` face au simulateur ; `rerender` change de modèle.
 * `prepare` remplace des gestionnaires avant le premier rendu.
 */
function setup(
  model: string | null,
  options: Parameters<typeof fakeBackend>[1] = {},
  prepare: (simulator: Simulator) => void = () => {},
) {
  const simulator = installSimulator({ latencyMs: 5 });
  const backend = fakeBackend(simulator, {
    models: [QWEN, GPT],
    details: [ON_OFF, LEVELS],
    ...options,
  });
  prepare(simulator);
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const hook = renderHook((name: string | null) => useThink(name), {
    wrapper,
    initialProps: model,
  });
  return { simulator, backend, client, ...hook };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 30));

describe("useThink", () => {
  it("combines the model's details and its remembered choice", async () => {
    const { result, simulator } = setup(QWEN, {
      thinkPrefs: { [QWEN]: false },
    });
    expect(result.current.control).toEqual({ kind: "none" });
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: false }),
    );
    expect(simulator.callsTo("model_details")).toEqual([{ model: QWEN }]);
    expect(simulator.callsTo("get_think_pref")).toEqual([{ model: QWEN }]);
  });

  it("without a remembered choice, Ollama's default", async () => {
    const { result } = setup(GPT);
    await waitFor(() =>
      expect(result.current.control).toMatchObject({
        kind: "levels",
        value: "medium",
      }),
    );
  });

  it("no button while the remembered choice is not known yet", async () => {
    let release = () => {};
    const { result, client } = setup(QWEN, {}, (simulator) =>
      simulator.handle(
        "get_think_pref",
        () => new Promise((resolve) => (release = () => resolve(false))),
      ),
    );
    await waitFor(() =>
      expect(client.getQueryState(modelDetailsKey(QWEN))?.status).toBe(
        "success",
      ),
    );
    await settle();
    // Détails connus, réglage pas encore : pas le défaut (true) en attendant.
    expect(result.current.control).toEqual({ kind: "none" });
    release();
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: false }),
    );
  });

  it("details in error (model not installed, Ollama down): no button", async () => {
    const { result, client } = setup("chatonlm-missing");
    await waitFor(() =>
      expect(
        client.getQueryState(modelDetailsKey("chatonlm-missing"))?.status,
      ).toBe("error"),
    );
    await settle();
    expect(result.current.control).toEqual({ kind: "none" });
  });

  it("known details survive a failed re-read, not a removed model", async () => {
    const { result, simulator, backend, client } = setup(QWEN, {
      thinkPrefs: { [QWEN]: false },
    });
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: false }),
    );
    let version = 0;
    const reread = async (answer: AppError) => {
      const before = simulator.callsTo("model_details").length;
      backend.details.set(QWEN, answer);
      // Liste changée à chaque relecture (sinon les détails ne sont pas
      // relus).
      version += 1;
      await act(() =>
        client.fetchQuery({
          queryKey: modelsKey,
          queryFn: () => [{ ...modelInfo(QWEN), digest: `v${version}` }],
        }),
      );
      await waitFor(() => {
        const state = client.getQueryState(modelDetailsKey(QWEN));
        expect(simulator.callsTo("model_details")).toHaveLength(before + 1);
        expect(state?.fetchStatus).toBe("idle");
        expect(state?.error).toEqual(answer);
      });
      await settle();
    };

    // Ollama lent une fois : le bouton reste, et Off avec lui.
    await reread({ kind: "ollama", message: "Ollama didn't answer in time" });
    expect(client.getQueryState(modelDetailsKey(QWEN))?.status).toBe("error");
    expect(result.current.control).toEqual({ kind: "toggle", value: false });

    // Modèle supprimé : plus de bouton, think non envoyé.
    await reread({ kind: "notFound", message: `model '${QWEN}' not found` });
    expect(result.current.control).toEqual({ kind: "none" });
  });

  it("an unreadable remembered choice falls back to the default", async () => {
    const { result } = setup(QWEN, {}, (simulator) =>
      simulator.handle("get_think_pref", () => {
        throw { kind: "database", message: "database is locked" };
      }),
    );
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: true }),
    );
  });

  it("no model: no button and no call", async () => {
    const { result, simulator } = setup(null);
    act(() => result.current.setThink(false));
    await settle();
    expect(result.current.control).toEqual({ kind: "none" });
    expect(simulator.calls).toEqual([]);
  });

  it("a change shows at once and is remembered for this model", async () => {
    const { result, simulator, backend } = setup(GPT);
    await waitFor(() =>
      expect(result.current.control).toMatchObject({ value: "medium" }),
    );
    let release = () => {};
    simulator.handle("set_think_pref", (args) => {
      backend.thinkPrefs.set(args.model as string, args.think as Think);
      return new Promise((resolve) => (release = () => resolve(null)));
    });
    act(() => result.current.setThink("high"));
    // Sans attendre la réponse du backend.
    await waitFor(() =>
      expect(result.current.control).toMatchObject({ value: "high" }),
    );
    expect(simulator.callsTo("set_think_pref")).toEqual([
      { model: GPT, think: "high" },
    ]);
    expect(backend.thinkPrefs.get(GPT)).toBe("high");
    expect(backend.thinkPrefs.has(QWEN)).toBe(false);
    release();
  });

  it("quick changes are saved in order: the last one stays", async () => {
    const { result, simulator, backend } = setup(QWEN);
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: true }),
    );
    // La première écriture est lente : sans file, la seconde finirait avant.
    let first = true;
    simulator.handle("set_think_pref", async (args) => {
      const slow = first;
      first = false;
      await new Promise((resolve) => setTimeout(resolve, slow ? 60 : 0));
      backend.thinkPrefs.set(args.model as string, args.think as Think);
      return null;
    });
    act(() => result.current.setThink(false));
    act(() => result.current.setThink(true));
    await waitFor(() =>
      expect(simulator.callsTo("set_think_pref")).toHaveLength(2),
    );
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(backend.thinkPrefs.get(QWEN)).toBe(true);
    expect(result.current.control).toEqual({ kind: "toggle", value: true });
  });

  it("another model has its own control and remembered choice", async () => {
    const { result, rerender } = setup(QWEN, {
      thinkPrefs: { [GPT]: "low" },
    });
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: true }),
    );
    act(() => result.current.setThink(false));

    rerender(GPT);
    // Pendant le chargement : rien de l'autre modèle (son Off irait à GPT).
    expect(result.current.control).toEqual({ kind: "none" });
    await waitFor(() =>
      expect(result.current.control).toMatchObject({
        kind: "levels",
        value: "low",
      }),
    );

    // Retour au premier : son choix à lui.
    rerender(QWEN);
    expect(result.current.control).toEqual({ kind: "toggle", value: false });
  });

  it.each(["model_details", "get_think_pref"])(
    "after a model change, nothing of the previous model while %s is pending",
    async (held) => {
      const { result, simulator, client, rerender } = setup(QWEN, {
        thinkPrefs: { [QWEN]: false, [GPT]: "low" },
      });
      await waitFor(() =>
        expect(result.current.control).toEqual({
          kind: "toggle",
          value: false,
        }),
      );
      // Seule l'autre lecture répond pour le nouveau modèle.
      let release = () => {};
      simulator.handle(
        held,
        () =>
          new Promise((resolve) => {
            release = () => resolve(held === "model_details" ? LEVELS : "low");
          }),
      );
      const other =
        held === "model_details" ? thinkPrefKey(GPT) : modelDetailsKey(GPT);

      rerender(GPT);
      await waitFor(() =>
        expect(client.getQueryState(other)?.status).toBe("success"),
      );
      await settle();
      expect(result.current.control).toEqual({ kind: "none" });

      release();
      await waitFor(() =>
        expect(result.current.control).toMatchObject({
          kind: "levels",
          value: "low",
        }),
      );
    },
  );

  it("an unchanged list of models reads nothing again", async () => {
    const { result, simulator, client } = setup(QWEN);
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: true }),
    );
    const list = () => [modelInfo(QWEN)];
    await act(() => client.fetchQuery({ queryKey: modelsKey, queryFn: list }));
    await settle();
    const after = simulator.callsTo("model_details").length;
    // Même liste relue (minuterie de 30 s) : pas d'appel à /api/show.
    await act(() =>
      client.fetchQuery({ queryKey: modelsKey, queryFn: list, staleTime: 0 }),
    );
    await settle();
    expect(simulator.callsTo("model_details")).toHaveLength(after);
  });

  it("details are read again each time the list of models changes", async () => {
    const { result, simulator, backend, client } = setup(QWEN);
    await waitFor(() =>
      expect(result.current.control).toEqual({ kind: "toggle", value: true }),
    );
    // Modèle retéléchargé : il a maintenant des niveaux.
    backend.details.set(
      QWEN,
      thinkingModel(QWEN, { values: ["low", "high"], default: "high" }),
    );
    await settle();
    expect(simulator.callsTo("model_details")).toHaveLength(1);

    await act(() =>
      client.fetchQuery({ queryKey: modelsKey, queryFn: () => [] }),
    );
    await waitFor(() =>
      expect(result.current.control).toMatchObject({
        kind: "levels",
        value: "high",
      }),
    );
    expect(simulator.callsTo("model_details")).toHaveLength(2);
  });
});
