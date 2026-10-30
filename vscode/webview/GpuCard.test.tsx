// Panneau, lot 3 (X) : carte « Graphics card » (autocomplétion, plan du chef
// de la carte graphique, modèles proposés, licence non commerciale du 3B,
// installation par l'app, préparation, mesure), ouverte par l'en-tête ou à la
// demande ponctuelle de l'hôte ; barre de mémoire (parts et légende, mémoire
// vive sans carte graphique) ; « Prepare » seulement quand il peut réussir ;
// autres complétions ; noms accessibles ; textes traduits.
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import type {
  CompletionView,
  HostMessage,
  PanelMessage,
  PanelState,
} from "../shared/protocol";
import { Panel } from "./Panel";

const CONNECTED: PanelState = {
  connection: "connected",
  models: ["qwen3:0.6b"],
  cloudModels: [],
  model: "qwen3:0.6b",
  ephemeral: false,
  chatId: null,
  busy: false,
  generation: 0,
  notice: null,
};

const GB = 1_000_000_000;

const VIEW: CompletionView = {
  enabled: true,
  status: "ready",
  reason: null,
  model: "qwen2.5-coder:1.5b-base",
  setting: "auto",
  otherProvider: null,
  alsoSuggests: null,
  error: null,
  plan: {
    mode: "together",
    chat: {
      model: "qwen3:0.6b",
      loaded: true,
      sizeBytes: 1 * GB,
      vramBytes: 1 * GB,
      estimatedBytes: null,
      partialGpu: false,
    },
    completion: {
      model: "qwen2.5-coder:1.5b-base",
      loaded: true,
      sizeBytes: 1.2 * GB,
      vramBytes: 1.2 * GB,
      estimatedBytes: null,
      partialGpu: true,
    },
    memory: { total: 8 * GB, free: 5 * GB, gpu: true },
    chatBusy: false,
  },
  recommended: "qwen2.5-coder:1.5b-base",
  options: [
    {
      model: "qwen2.5-coder:0.5b-base",
      installed: true,
      license: "Apache-2.0",
      commercialUse: true,
      fits: true,
      estimatedBytes: GB,
    },
    {
      model: "qwen2.5-coder:1.5b-base",
      installed: true,
      license: "Apache-2.0",
      commercialUse: true,
      fits: true,
      estimatedBytes: GB,
    },
    {
      model: "qwen2.5-coder:3b-base",
      installed: false,
      license: "Qwen Research",
      commercialUse: false,
      fits: true,
      estimatedBytes: GB,
    },
    {
      model: "qwen2.5-coder:7b-base",
      installed: false,
      license: "Apache-2.0",
      commercialUse: true,
      fits: false,
      estimatedBytes: 5 * GB,
    },
  ],
  preparing: false,
  measure: null,
};

afterEach(() => cleanup());

function fromHost(data: HostMessage) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

function renderPanel(state: Partial<PanelState> = {}) {
  const post = vi.fn<(message: PanelMessage) => void>();
  render(<Panel post={post} />);
  fromHost({
    type: "init",
    locale: "en",
    theme: "light",
    state: { ...CONNECTED, ...state },
  });
  return post;
}

const posted = (post: ReturnType<typeof renderPanel>) =>
  post.mock.calls.map(([message]) => message).filter((m) => m.type !== "ready");

const openCard = () =>
  fireEvent.click(screen.getByRole("button", { name: "Graphics card" }));

/** Vue de l'hôte, carte montrée à sa demande. */
function shown(view: Partial<CompletionView> = {}) {
  fromHost({ type: "completion", completion: { ...VIEW, ...view } });
  fromHost({ type: "revealGpu" });
}

/** Largeur d'une part de la barre de mémoire (absente : 0 %). */
const partWidth = (id: string) =>
  screen.queryByTestId(`gpu-part-${id}`)?.style.width ?? "0%";

describe("Graphics card", () => {
  it("opens from the header, and reads the plan again", () => {
    const post = renderPanel();
    fromHost({ type: "completion", completion: VIEW });
    expect(screen.queryByTestId("gpu")).toBeNull();
    openCard();
    expect(posted(post)).toEqual([{ type: "completion", command: "refresh" }]);
    const card = screen.getByTestId("gpu");
    expect(
      screen.getByRole("button", { name: "Graphics card" }).ariaPressed,
    ).toBe("true");
    expect(screen.getByTestId("gpu-status").textContent).toBe(
      "Ready, with qwen2.5-coder:1.5b-base.",
    );
    expect(within(card).getByText("Chat and autocomplete fit together"));
    expect(screen.getByTestId("gpu-memory").textContent).toBe(
      "Graphics memory · 2.8 GB of 7.5 GB used",
    );
    expect(screen.getByRole("meter").getAttribute("aria-valuenow")).toBe("38");
    expect(screen.getByTestId("gpu-slot-chat").textContent).toBe(
      "Chat · qwen3:0.6b · 954 MB · in memory",
    );
    expect(screen.getByTestId("gpu-slot-completion").textContent).toContain(
      "partly on the processor",
    );
    expect(screen.getByTestId("gpu-other").textContent).toBe(
      "Other use · 763 MB",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Close the graphics card" }),
    );
    expect(screen.queryByTestId("gpu")).toBeNull();
    // Le focus revient au bouton qui rouvre la carte.
    expect(document.activeElement).toBe(screen.getByTestId("gpu-button"));
  });

  it("says that the 3B is for non-commercial use, and installs through the app", () => {
    const post = renderPanel();
    fromHost({ type: "completion", completion: VIEW });
    openCard();
    const row = (model: string) =>
      screen
        .getAllByTestId("gpu-option")
        .find((li) => li.dataset.model === model) as HTMLElement;
    const threeB = row("qwen2.5-coder:3b-base");
    expect(within(threeB).getByTestId("gpu-non-commercial").textContent).toBe(
      "Non-commercial use only",
    );
    expect(
      within(row("qwen2.5-coder:1.5b-base")).queryByTestId(
        "gpu-non-commercial",
      ),
    ).toBeNull();
    expect(row("qwen2.5-coder:7b-base").textContent).toContain(
      "May not fit on this computer",
    );
    expect(row("qwen2.5-coder:1.5b-base").textContent).toContain("In use");
    expect(row("qwen2.5-coder:1.5b-base").textContent).toContain("Recommended");
    fireEvent.click(
      within(threeB).getByRole("button", {
        name: "Install qwen2.5-coder:3b-base in ChatonLM",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Use qwen2.5-coder:0.5b-base" }),
    );
    expect(posted(post).slice(1)).toEqual([
      { type: "offerPull", model: "qwen2.5-coder:3b-base" },
      { type: "completionModel", model: "qwen2.5-coder:0.5b-base" },
    ]);
  });

  it("turns autocomplete on and off, prepares and measures", () => {
    const post = renderPanel();
    fromHost({
      type: "completion",
      completion: { ...VIEW, enabled: false, status: "off" },
    });
    openCard();
    // Nom stable, l'état est dans aria-checked.
    const toggle = screen.getByRole("switch", { name: "Autocomplete" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    // En pause (modèle pas chargé) : « Prepare », pas de mesure.
    fromHost({
      type: "completion",
      completion: { ...VIEW, status: "paused", reason: "notLoaded" },
    });
    expect(screen.getByTestId("gpu-status").textContent).toBe(
      "Paused: the autocomplete model isn't loaded. Prepare it to start.",
    );
    expect(screen.queryByRole("button", { name: "Measure speed" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Prepare" }));
    fromHost({ type: "completion", completion: VIEW });
    expect(screen.queryByRole("button", { name: "Prepare" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Measure speed" }));
    fromHost({
      type: "completion",
      completion: {
        ...VIEW,
        measure: {
          state: "done",
          result: {
            rating: "usable",
            firstTokenMs: 640.4,
            tokensPerSecond: 31.6,
            partialGpu: true,
          },
        },
      },
    });
    expect(screen.getByTestId("gpu-measure").textContent).toBe(
      "Usable, a little slowFirst word in 640 ms, 32 tokens per second. Part of the model runs on the processor.",
    );
    const on = screen.getByRole("switch", { name: "Autocomplete" });
    expect(on.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(on);
    expect(posted(post).slice(1)).toEqual([
      { type: "completion", command: "enable" },
      { type: "completion", command: "prepare" },
      { type: "completion", command: "measure" },
      { type: "completion", command: "disable" },
    ]);
  });

  it("opens when the host asks, and stays closed after that", () => {
    renderPanel();
    fromHost({ type: "completion", completion: VIEW });
    expect(screen.queryByTestId("gpu")).toBeNull();
    fromHost({ type: "revealGpu" });
    expect(screen.getByTestId("gpu")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Close the graphics card" }),
    );
    fromHost({
      type: "completion",
      completion: { ...VIEW, status: "paused", reason: "chatBusy" },
    });
    expect(screen.queryByTestId("gpu")).toBeNull();
    fromHost({ type: "revealGpu" });
    expect(screen.getByTestId("gpu")).toBeTruthy();
  });

  it("does not open by itself in a panel opened again later", () => {
    renderPanel();
    // Panneau recréé : l'hôte redonne l'état, sans demande de le montrer.
    fromHost({ type: "completion", completion: VIEW });
    expect(screen.queryByTestId("gpu")).toBeNull();
  });

  it("says memory, not graphics card, without a graphics card; free memory may be unknown", () => {
    renderPanel();
    const plan = VIEW.plan!;
    // Sans carte reconnue, Ollama ne met rien en VRAM : les parts sont les
    // tailles des modèles en mémoire vive.
    const inRam = (slot: typeof plan.chat) => slot && { ...slot, vramBytes: 0 };
    shown({
      plan: {
        ...plan,
        chat: inRam(plan.chat),
        completion: inRam(plan.completion),
        memory: { total: 16 * GB, free: 10 * GB, gpu: false },
      },
    });
    expect(screen.getByTestId("gpu-memory").textContent).toBe(
      "Memory · 5.6 GB of 15 GB used",
    );
    expect(partWidth("chat")).toBe("6.25%");
    expect(partWidth("completion")).toBe("7.5%");
    expect(partWidth("other")).toBe("23.75%");
    expect(screen.getByRole("meter", { name: "Memory" })).toBeTruthy();
    expect(
      screen.getByText(
        "No graphics card found: models run in the computer's memory.",
      ),
    ).toBeTruthy();
    fromHost({
      type: "completion",
      completion: {
        ...VIEW,
        plan: { ...plan, memory: { total: 8 * GB, free: null, gpu: true } },
      },
    });
    expect(screen.getByTestId("gpu-memory").textContent).toBe(
      "Graphics memory · 7.5 GB in all",
    );
    expect(partWidth("chat")).toBe("12.5%");
    expect(screen.queryByTestId("gpu-other")).toBeNull();
    expect(screen.queryByText(/No graphics card found/)).toBeNull();
  });

  it("tells the parts of the memory bar apart, with a legend", () => {
    renderPanel();
    shown();
    const parts = ["chat", "completion", "other"].map(
      (id) => screen.getByTestId(`gpu-part-${id}`).className,
    );
    // Trois couleurs, et un filet de la couleur de la carte entre deux parts.
    expect(new Set(parts.map((c) => c.match(/ bg-\S+/)?.[0])).size).toBe(3);
    for (const part of parts) expect(part).toContain("border-r-2");
    // Même pastille devant chaque nom que sa part dans la barre.
    const dot = (testId: string) =>
      screen.getByTestId(testId).querySelector("[aria-hidden=true]")
        ?.className ?? "";
    expect(dot("gpu-slot-chat")).toContain(parts[0].split(" ").at(-1));
    expect(dot("gpu-slot-completion")).toContain("bg-accent");
    expect(dot("gpu-other")).toContain(parts[2].split(" ").at(-1));
  });

  it("offers Prepare only when it can work", () => {
    renderPanel();
    const prepare = () => screen.queryByRole("button", { name: "Prepare" });
    shown({ status: "paused", reason: "notLoaded" });
    expect(prepare()).toBeTruthy();
    for (const reason of [
      "doesNotFit",
      "notInstalled",
      "chatBusy",
      "notLocal",
      "exclusive",
    ] as const) {
      fromHost({
        type: "completion",
        completion: { ...VIEW, status: "paused", reason },
      });
      expect(prepare(), reason).toBeNull();
    }
    // L'un après l'autre, la conversation pas en mémoire : ça peut réussir.
    fromHost({
      type: "completion",
      completion: {
        ...VIEW,
        status: "paused",
        reason: "exclusive",
        plan: {
          ...VIEW.plan!,
          mode: "exclusive",
          chat: { ...VIEW.plan!.chat!, loaded: false },
        },
      },
    });
    expect(prepare()).toBeTruthy();
  });

  it("says another extension may also suggest, and offers to suggest anyway", () => {
    const post = renderPanel();
    shown({ alsoSuggests: "GitHub Copilot" });
    expect(screen.getByTestId("gpu-also").textContent).toBe(
      "GitHub Copilot may also suggest code here.",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Always suggest with ChatonLM" }),
    );
    expect(posted(post).at(-1)).toEqual({
      type: "completion",
      command: "ignoreOthers",
    });
    fromHost({
      type: "completion",
      completion: {
        ...VIEW,
        status: "otherProvider",
        otherProvider: "GitHub Copilot",
      },
    });
    expect(screen.queryByTestId("gpu-also")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Always suggest with ChatonLM" }),
    ).toBeTruthy();
    fromHost({ type: "completion", completion: VIEW });
    expect(
      screen.queryByRole("button", { name: "Always suggest with ChatonLM" }),
    ).toBeNull();
  });

  it("marks nothing as too big on an unknown computer, and shows the app's refusal", () => {
    renderPanel();
    shown({ options: VIEW.options.map((o) => ({ ...o, fits: null })) });
    expect(screen.queryByText(/May not fit/)).toBeNull();
    expect(screen.getAllByTestId("gpu-option")[0].textContent).toContain(
      "954 MB",
    );
    fromHost({
      type: "completion",
      completion: {
        ...VIEW,
        status: "unusable",
        error: "This model can't complete code.",
      },
    });
    expect(screen.getByTestId("gpu-status").textContent).toBe(
      "This model can't complete code.",
    );
  });

  it("is hidden while ChatonLM is not connected", () => {
    renderPanel({ connection: "appClosed" });
    shown();
    expect(screen.queryByTestId("gpu")).toBeNull();
    expect(
      (
        screen.getByRole("button", {
          name: "Graphics card",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });

  it("has every text translated", () => {
    usePseudoLanguage();
    const allow = [
      "qwen3:0.6b",
      "qwen2.5-coder:0.5b-base",
      "qwen2.5-coder:1.5b-base",
      "qwen2.5-coder:3b-base",
      "qwen2.5-coder:7b-base",
      "Apache-2.0",
      "Qwen Research",
      "GitHub Copilot",
    ];
    renderPanel();
    shown({
      alsoSuggests: "GitHub Copilot",
      measure: {
        state: "done",
        result: {
          rating: "smooth",
          firstTokenMs: 100,
          tokensPerSecond: 80,
          partialGpu: true,
        },
      },
    });
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    const views: Partial<CompletionView>[] = [
      { enabled: false, status: "off" },
      { status: "disconnected" },
      { status: "notHere" },
      { status: "remoteWindow" },
      { status: "slow" },
      { status: "otherProvider", otherProvider: "GitHub Copilot" },
      { status: "notLocal" },
      { status: "missing" },
      { status: "checking" },
      ...(
        [
          "exclusive",
          "chatBusy",
          "doesNotFit",
          "notInstalled",
          "notLocal",
          "notLoaded",
        ] as const
      ).map((reason) => ({ status: "paused" as const, reason })),
      { preparing: true, status: "paused" },
      { measure: { state: "running" } },
      {
        plan: {
          ...VIEW.plan!,
          memory: { total: 8 * GB, free: null, gpu: false },
        },
      },
    ];
    for (const mode of ["exclusive", "doesNotFit", "unknown"] as const) {
      views.push({ plan: { ...VIEW.plan!, mode } });
    }
    for (const rating of ["usable", "tooSlow"] as const) {
      views.push({
        measure: {
          state: "done",
          result: {
            rating,
            firstTokenMs: 1,
            tokensPerSecond: 1,
            partialGpu: false,
          },
        },
      });
    }
    for (const view of views) {
      fromHost({
        type: "completion",
        completion: { ...VIEW, ...view },
      });
      expect(
        untranslatedTexts(document.body, allow),
        JSON.stringify(view),
      ).toEqual([]);
    }
  });
});
