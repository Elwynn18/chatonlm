// Modèles recommandés du sélecteur : leur mémoire dépend du contexte réglé,
// la liste est relue quand il change (pas pour un autre réglage).

import { QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { useRecommendedModels } from "@/hooks/useModels";
import { useSettings } from "@/hooks/useSettings";
import { createQueryClient } from "@/lib/queryClient";
import { fakeBackend } from "@/test/backend";
import { installSimulator } from "@/test/simulator";

afterEach(() => cleanup());

describe("useRecommendedModels", () => {
  it("reads the list again when the context length changes", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator, { recommended: ["unsloth/Qwen3-8B-GGUF:Q4_K_M"] });
    const client = createQueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(
      () => ({ list: useRecommendedModels(), settings: useSettings() }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.list).toHaveLength(1));
    const reads = () => simulator.callsTo("recommended_models").length;
    expect(reads()).toBe(1);
    await act(() => result.current.settings.setSettings({ theme: "dark" }));
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(reads()).toBe(1);
    await act(() =>
      result.current.settings.setSettings({ contextLength: 131072 }),
    );
    await waitFor(() => expect(reads()).toBe(2));
    client.clear();
  });
});
