// Réglages avancés de l'image (bouton à côté du sélecteur du modèle d'images) :
// rien sans modèle choisi ; panneau : champs vides = valeurs du modèle
// (rappelées), enregistrement à la sortie d'un champ, CFG à virgule, prompt
// négatif gardé même si Échap ferme le panneau en pleine saisie, graine fixe,
// valeurs refusées sans enregistrement, modèle sans guidage (ni CFG ni prompt
// négatif, phrase qui le dit), « Model values », pseudo-langue.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ImageSettings } from "@/bindings/ImageSettings";
import type { ImageSettingsView } from "@/bindings/ImageSettingsView";
import { setLanguage } from "@/i18n";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { parseCfg, parseWhole } from "@/lib/imageSettings";
import { ImageSettingsButton } from "./ImageSettingsButton";

vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  cleanup();
  clearMocks();
});

const CHROMA_ID = "silveroxides/Chroma-GGUF/Chroma1-HD/Chroma1-HD-Q4_0.gguf";
const EMPTY: ImageSettings = {
  steps: null,
  cfgScale: null,
  negativePrompt: "",
  seed: null,
};

function view(fields: Partial<ImageSettingsView> = {}): ImageSettingsView {
  return {
    modelId: CHROMA_ID,
    modelName: "Chroma Q4_0",
    settings: EMPTY,
    defaultSteps: 26,
    defaultCfgScale: 4,
    guidance: true,
    maxSteps: 150,
    maxPrompt: 8000,
    ...fields,
  };
}

/** Faux moteur : réglages rendus par image_settings, enregistrements notés. */
function setup(initial: ImageSettingsView | null) {
  const state = { view: initial };
  const saved: { id: string; settings: ImageSettings }[] = [];
  mockIPC((command, args) => {
    switch (command) {
      case "image_settings":
        return state.view;
      case "image_set_settings": {
        const { id, settings } = args as {
          id: string;
          settings: ImageSettings;
        };
        saved.push({ id, settings });
        if (state.view) state.view = { ...state.view, settings };
        return null;
      }
    }
    throw new Error(`unexpected command ${command}`);
  });
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <p>Outside</p>
      <ImageSettingsButton />
    </QueryClientProvider>,
  );
  return { saved };
}

const openPanel = async () => {
  await userEvent.click(
    await screen.findByRole("button", { name: "Image settings" }),
  );
  return screen.getByRole("dialog", { name: "Image settings" });
};

describe("image settings", () => {
  it("no image model chosen: no button", async () => {
    setup(null);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("button", { name: "Image settings" })).toBeNull();
  });

  it("empty fields mean the model's values; each change is saved when its field is left", async () => {
    const { saved } = setup(view());
    const panel = await openPanel();
    expect(panel.textContent).toContain("For Chroma Q4_0");
    const steps = screen.getByLabelText("Steps") as HTMLInputElement;
    const cfg = screen.getByLabelText("Guidance (CFG)") as HTMLInputElement;
    expect(steps.value).toBe("");
    expect(steps.placeholder).toBe("26");
    expect(cfg.placeholder).toBe("4");
    expect(panel.textContent).toContain("Model value: 26.");
    await userEvent.type(steps, "30");
    expect(saved).toEqual([]);
    await userEvent.tab();
    await waitFor(() =>
      expect(saved).toEqual([
        { id: CHROMA_ID, settings: { ...EMPTY, steps: 30 } },
      ]),
    );
    // Virgule acceptée (claviers français).
    await userEvent.type(cfg, "3,5");
    await userEvent.tab();
    await waitFor(() =>
      expect(saved.at(-1)!.settings).toEqual({
        ...EMPTY,
        steps: 30,
        cfgScale: 3.5,
      }),
    );
    // Graine fixe.
    await userEvent.click(screen.getByLabelText("Fixed"));
    await userEvent.type(screen.getByLabelText("Seed value"), "42");
    await userEvent.tab();
    await waitFor(() => expect(saved.at(-1)!.settings.seed).toBe(42));
    // Revenue à aléatoire : enregistrée tout de suite.
    await userEvent.click(screen.getByLabelText("Random"));
    await waitFor(() => expect(saved.at(-1)!.settings.seed).toBeNull());
  });

  it("a negative prompt being typed is kept when Escape closes the panel", async () => {
    const { saved } = setup(view());
    await openPanel();
    await userEvent.type(
      screen.getByLabelText("Negative prompt"),
      "blurry, extra fingers",
    );
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() =>
      expect(saved).toEqual([
        {
          id: CHROMA_ID,
          settings: { ...EMPTY, negativePrompt: "blurry, extra fingers" },
        },
      ]),
    );
    // Rouvert : le prompt négatif enregistré.
    await openPanel();
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Negative prompt") as HTMLTextAreaElement).value,
      ).toBe("blurry, extra fingers"),
    );
  });

  it("refused values are shown and never saved", async () => {
    const { saved } = setup(view());
    const panel = await openPanel();
    await userEvent.type(screen.getByLabelText("Steps"), "0");
    await userEvent.tab();
    expect(panel.textContent).toContain("A whole number between 1 and 150.");
    expect(screen.getByLabelText("Steps").getAttribute("aria-invalid")).toBe(
      "true",
    );
    await userEvent.type(screen.getByLabelText("Guidance (CFG)"), "31");
    await userEvent.tab();
    expect(panel.textContent).toContain("A number between 0 and 30.");
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(saved).toEqual([]);
  });

  it("a model without guidance: no CFG nor negative prompt, a sentence says why", async () => {
    setup(view({ defaultSteps: 4, defaultCfgScale: 1, guidance: false }));
    await openPanel();
    expect(screen.queryByLabelText("Guidance (CFG)")).toBeNull();
    expect(screen.queryByLabelText("Negative prompt")).toBeNull();
    expect(screen.getByTestId("image-settings-no-guidance").textContent).toBe(
      "This model doesn't use guidance: the CFG and a negative prompt would have no effect on it.",
    );
    expect(
      (screen.getByLabelText("Steps") as HTMLInputElement).placeholder,
    ).toBe("4");
  });

  it("Model values clears every setting", async () => {
    const { saved } = setup(
      view({
        settings: {
          steps: 40,
          cfgScale: 5.5,
          negativePrompt: "text",
          seed: 7,
        },
      }),
    );
    await openPanel();
    expect((screen.getByLabelText("Steps") as HTMLInputElement).value).toBe(
      "40",
    );
    expect(
      (screen.getByLabelText("Guidance (CFG)") as HTMLInputElement).value,
    ).toBe("5.5");
    expect((screen.getByLabelText("Fixed") as HTMLInputElement).checked).toBe(
      true,
    );
    await userEvent.click(screen.getByRole("button", { name: "Model values" }));
    await waitFor(() =>
      expect(saved).toEqual([{ id: CHROMA_ID, settings: EMPTY }]),
    );
    expect((screen.getByLabelText("Steps") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText("Random") as HTMLInputElement).checked).toBe(
      true,
    );
  });

  it("French: the CFG is written with a comma", async () => {
    await setLanguage("fr");
    try {
      setup(view({ settings: { ...EMPTY, cfgScale: 3.5 } }));
      await userEvent.click(
        await screen.findByRole("button", { name: "Réglages de l'image" }),
      );
      expect(
        (screen.getByLabelText("Guidage (CFG)") as HTMLInputElement).value,
      ).toBe("3,5");
    } finally {
      await setLanguage("en");
    }
  });

  it("pseudo-language: no text written in the code", async () => {
    usePseudoLanguage();
    setup(view());
    await userEvent.click(
      await screen.findByRole("button", { name: "⟦Image settings⟧" }),
    );
    expect(untranslatedTexts(document.body, ["Outside"])).toEqual([]);
  });
});

describe("image settings: reading the fields", () => {
  it("whole numbers within bounds; empty is the model's value", () => {
    expect(parseWhole("", 1, 150)).toBeNull();
    expect(parseWhole(" 30 ", 1, 150)).toBe(30);
    expect(parseWhole("0", 1, 150)).toBeUndefined();
    expect(parseWhole("151", 1, 150)).toBeUndefined();
    expect(parseWhole("2.5", 1, 150)).toBeUndefined();
    expect(parseWhole("-3", 0, 10)).toBeUndefined();
  });

  it("CFG with a dot or a comma, between 0 and 30", () => {
    expect(parseCfg("")).toBeNull();
    expect(parseCfg("4")).toBe(4);
    expect(parseCfg("3,5")).toBe(3.5);
    expect(parseCfg("3.5")).toBe(3.5);
    expect(parseCfg("30")).toBe(30);
    expect(parseCfg("30.5")).toBeUndefined();
    expect(parseCfg("abc")).toBeUndefined();
  });
});
