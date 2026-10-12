// Page Découvrir du ModelHub face au simulateur : accueil, machine, filtres
// (commandes et adresse), rubriques et familles, recherche différée et page
// par page, raccourcis, états de Hugging Face.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { HubFilters } from "@/bindings/HubFilters";
import type { HubSearch } from "@/bindings/HubSearch";
import type { Settings } from "@/bindings/Settings";
import { settingsKey } from "@/hooks/useSettings";
import { renderHub } from "@/test/renderHub";
import type { Simulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const NO_FILTERS: HubFilters = {
  uses: [],
  size: null,
  openLicense: false,
  verifiedOnly: false,
};

const delay = (ms: number) =>
  act(() => new Promise((resolve) => setTimeout(resolve, ms)));

/** Accueil affiché. */
async function home() {
  await screen.findByRole("heading", { name: "Trending" });
}

const homeFilters = (simulator: Simulator) =>
  simulator.callsTo("hub_home").map((args) => args.filters as HubFilters);

const searches = (simulator: Simulator) =>
  simulator.callsTo("hub_search").map((args) => args.search as HubSearch);

const searchField = () =>
  screen.getByRole("textbox", { name: "Search Hugging Face models" });

const chip = (name: string) => screen.getByRole("button", { name });

/** Première carte d'un modèle (il peut figurer dans plusieurs rubriques). */
const card = (name: string) =>
  screen
    .getAllByTestId("model-card")
    .find((element) => element.querySelector("h4")?.textContent === name)!;

const cardNames = () =>
  screen
    .getAllByTestId("model-card")
    .map((element) => element.querySelector("h4")?.textContent);

describe("Discover: home", () => {
  it("shows the title, the machine, the sections in order and the families", async () => {
    const { simulator } = renderHub("/hub");
    await home();
    expect(
      screen.getByRole("heading", { name: "Discover models" }),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Open models from Hugging Face, ready to run with Ollama.",
      ),
    ).toBeTruthy();
    const machine = screen.getByTestId("machine");
    expect(machine.textContent).toBe("RTX 3090 · 24 GB VRAM · 32 GB RAM");
    expect(machine.getAttribute("title")).toContain("Fits your GPU");
    expect(
      screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent),
    ).toEqual([
      "Best for your machine",
      "Trending",
      "Families",
      "Popular",
      "For coding",
      "With vision",
      "Reasoning",
      "Small and fast",
    ]);
    expect(homeFilters(simulator)).toEqual([NO_FILTERS]);

    const qwen = card("Qwen3.5-9B");
    expect(qwen.getAttribute("href")).toBe(
      "/hub/model/unsloth/Qwen3.5-9B-GGUF",
    );
    expect(within(qwen).getByText("unsloth")).toBeTruthy();
    expect(within(qwen).getByText("Verified")).toBeTruthy();
    expect(within(qwen).getByText("9.7B")).toBeTruthy();
    expect(within(qwen).getByText("256K")).toBeTruthy();
    expect(within(qwen).getByText("1.9M")).toBeTruthy();
    expect(within(qwen).getByText("Fits your GPU · 7.5 GB")).toBeTruthy();
    expect(
      within(qwen)
        .getAllByRole("img")
        .map((icon) => icon.getAttribute("aria-label")),
    ).toEqual(["Tools", "Reasoning", "Vision"]);
    expect(
      within(card("Qwen3.8-27B")).getByText(/^Runs partly on CPU/),
    ).toBeTruthy();

    const families = screen.getByRole("region", { name: "Families" });
    const qwenFamily = within(families).getByRole("link", { name: /Qwen/ });
    expect(qwenFamily.textContent).toContain("4 models");
    expect(
      within(families).getByRole("link", { name: /gpt-oss/ }).textContent,
    ).toContain("1 model");
  });

  it("marks the cards of installed repositories, without case", async () => {
    renderHub("/hub", ({ backend }) => {
      backend.models.unshift("hf.co/UNSLOTH/qwen3.5-9b-gguf:Q4_K_M");
    });
    await home();
    await waitFor(() =>
      expect(within(card("Qwen3.5-9B")).queryByText("Installed")).toBeTruthy(),
    );
    expect(within(card("gpt-oss-20b")).queryByText("Installed")).toBeNull();
  });

  it("a remote server: no fit, no Fits chip, sizes only", async () => {
    renderHub("/hub", ({ hub }) => {
      hub.hardware = null;
    });
    await screen.findByRole("heading", { name: "Trending" });
    expect(screen.getByTestId("machine").textContent).toBe(
      "Remote server — memory fit unknown",
    );
    expect(
      screen.queryByRole("button", { name: "Fits my machine" }),
    ).toBeNull();
    expect(
      screen.queryByRole("heading", { name: "Best for your machine" }),
    ).toBeNull();
    const pill = card("Qwen3.5-9B").querySelector("[data-fit]")!;
    expect(pill.getAttribute("data-fit")).toBe("unknown");
    expect(pill.textContent).toBe("7.5 GB");
  });

  it("reads the catalog and the machine again when the server or the context length changes", async () => {
    const { simulator, app } = renderHub("/hub");
    await home();
    const counts = () =>
      ["hub_home", "hub_hardware", "hub_installed"].map(
        (command) => simulator.callsTo(command).length,
      );
    await waitFor(() => expect(counts()).toEqual([1, 1, 1]));
    const settings = app.queryClient.getQueryData<Settings>(settingsKey)!;
    // Longueur de contexte : estimations (pastilles) relues.
    act(() =>
      app.queryClient.setQueryData(settingsKey, {
        ...settings,
        contextLength: 8192,
      }),
    );
    await waitFor(() => expect(counts().slice(0, 2)).toEqual([2, 2]));
    // Autre serveur : tout est relu, installés compris.
    act(() =>
      app.queryClient.setQueryData(settingsKey, {
        ...settings,
        contextLength: 8192,
        effectiveServerUrl: "http://192.168.1.20:11434",
      }),
    );
    await waitFor(() => expect(counts()).toEqual([3, 3, 2]));
    await home();
  });

  it("shows skeletons while the catalog loads", async () => {
    const { simulator } = renderHub("/hub");
    simulator.handle("hub_home", () => new Promise(() => {}));
    expect(
      await screen.findByRole("status", { name: "Loading models" }),
    ).toBeTruthy();
  });
});

describe("Discover: filters", () => {
  it("send the filters to the backend and keep them in the address", async () => {
    const { simulator, app } = renderHub("/hub");
    await home();
    expect(screen.queryByRole("button", { name: "Reset filters" })).toBeNull();

    await userEvent.click(chip("Code"));
    await waitFor(() =>
      expect(homeFilters(simulator).at(-1)).toEqual({
        ...NO_FILTERS,
        uses: ["code"],
      }),
    );
    expect(app.router.state.location.search).toEqual({ uses: "code" });
    expect(chip("Code").getAttribute("aria-pressed")).toBe("true");

    await userEvent.click(chip("Tools"));
    await userEvent.click(chip("Medium"));
    await userEvent.click(chip("Open license"));
    await waitFor(() =>
      expect(homeFilters(simulator).at(-1)).toEqual({
        uses: ["code", "tools"],
        size: "medium",
        openLicense: true,
        verifiedOnly: false,
      }),
    );
    expect(app.router.state.location.search).toEqual({
      uses: "code,tools",
      size: "medium",
      license: "open",
    });
    // Une taille à la fois ; un second clic la retire.
    await userEvent.click(chip("Small"));
    await waitFor(() =>
      expect(homeFilters(simulator).at(-1)?.size).toBe("small"),
    );
    expect(chip("Medium").getAttribute("aria-pressed")).toBe("false");
    await userEvent.click(chip("Small"));
    await waitFor(() => expect(homeFilters(simulator).at(-1)?.size).toBeNull());

    // Retour sans filtre : la réponse du début, encore fraîche, est reprise.
    await userEvent.click(
      screen.getByRole("button", { name: "Reset filters" }),
    );
    await waitFor(() => expect(app.router.state.location.search).toEqual({}));
    for (const name of ["Code", "Tools", "Small", "Open license"]) {
      expect(chip(name).getAttribute("aria-pressed")).toBe("false");
    }
    expect(screen.queryByRole("button", { name: "Reset filters" })).toBeNull();
  });

  it("read from the address", async () => {
    const { simulator } = renderHub(
      "/hub?uses=tools%2Ccode&size=fits&license=open&verified=true",
    );
    await screen.findByRole("heading", { name: "For coding" });
    // « Verified only » ne vaut que pour une recherche.
    expect(homeFilters(simulator)).toEqual([
      {
        uses: ["code", "tools"],
        size: "fits",
        openLicense: true,
        verifiedOnly: false,
      },
    ]);
    expect(chip("Code").getAttribute("aria-pressed")).toBe("true");
    expect(chip("Tools").getAttribute("aria-pressed")).toBe("true");
    expect(chip("Vision").getAttribute("aria-pressed")).toBe("false");
    expect(chip("Fits my machine").getAttribute("aria-pressed")).toBe("true");
    expect(screen.queryByRole("button", { name: "Verified only" })).toBeNull();
  });

  it("no match: the filters can be reset from the empty state", async () => {
    const { simulator } = renderHub("/hub?uses=code%2Cvision%2Creasoning");
    expect(
      await screen.findByText("No models match these filters."),
    ).toBeTruthy();
    const empty = screen.getByTestId("hub-empty");
    await userEvent.click(
      within(empty).getByRole("button", { name: "Reset filters" }),
    );
    await home();
    expect(homeFilters(simulator).at(-1)).toEqual(NO_FILTERS);
  });

  it("form a toolbar: one tab stop, arrows, Home and End", async () => {
    renderHub("/hub");
    await home();
    const toolbar = screen.getByRole("toolbar", { name: "Filters" });
    const buttons = within(toolbar).getAllByRole("button");
    expect(buttons.map((b) => b.tabIndex)).toEqual([
      0,
      ...buttons.slice(1).map(() => -1),
    ]);
    await userEvent.click(searchField());
    await userEvent.tab();
    expect(document.activeElement).toBe(chip("Code"));
    await userEvent.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(chip("Vision"));
    expect(chip("Vision").tabIndex).toBe(0);
    expect(chip("Code").tabIndex).toBe(-1);
    await userEvent.keyboard("{End}");
    expect(document.activeElement).toBe(chip("Open license"));
    await userEvent.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(chip("Code"));
    await userEvent.keyboard("{ArrowLeft}");
    expect(document.activeElement).toBe(chip("Open license"));
    await userEvent.keyboard("{Home}");
    expect(document.activeElement).toBe(chip("Code"));
    // Espace bascule la puce qui a le focus.
    await userEvent.keyboard(" ");
    expect(chip("Code").getAttribute("aria-pressed")).toBe("true");
  });
});

describe("Discover: sections and families", () => {
  it("See all shows the full section; All models comes back", async () => {
    const { simulator, app } = renderHub("/hub");
    await home();
    await userEvent.click(
      screen.getByRole("link", { name: "See all Small and fast" }),
    );
    await screen.findByRole("heading", { name: "Small and fast", level: 3 });
    expect(app.router.state.location.search).toEqual({ section: "small" });
    expect(simulator.callsTo("hub_browse")).toEqual([
      { scope: { kind: "section", id: "small" }, filters: NO_FILTERS },
    ]);
    await waitFor(() =>
      expect(cardNames()).toEqual([
        "gemma-3-270m-it",
        "LFM2-1.2B",
        "Phi-4-mini-reasoning",
        "SmolLM3-3B",
      ]),
    );
    expect(screen.getByText("4 models")).toBeTruthy();
    // Pas de recherche proposée pour une rubrique.
    expect(
      screen.queryByRole("link", { name: /Search Hugging Face for/ }),
    ).toBeNull();
    await userEvent.click(screen.getByRole("link", { name: "All models" }));
    await home();
    expect(app.router.state.location.search).toEqual({});
  });

  it("a family tile opens the family, which offers a search on Hugging Face", async () => {
    const { simulator, app } = renderHub("/hub");
    await home();
    const families = screen.getByRole("region", { name: "Families" });
    await userEvent.click(within(families).getByRole("link", { name: /Qwen/ }));
    await screen.findByRole("heading", { name: "Qwen", level: 3 });
    expect(simulator.callsTo("hub_browse").at(-1)).toEqual({
      scope: { kind: "family", id: "qwen" },
      filters: NO_FILTERS,
    });
    await waitFor(() => expect(cardNames()).toHaveLength(4));
    await userEvent.click(
      screen.getByRole("link", { name: "Search Hugging Face for “Qwen”" }),
    );
    await screen.findByRole("heading", { name: "Results for “Qwen”" });
    expect(app.router.state.location.search).toEqual({ q: "Qwen" });
    // L'adresse remplit le champ.
    expect(searchField()).toHaveProperty("value", "Qwen");
    expect(searches(simulator).at(-1)).toEqual({
      text: "Qwen",
      filters: NO_FILTERS,
      cursor: null,
    });
  });
});

describe("Discover: search", () => {
  it("searches 400 ms after the last keystroke, from two characters", async () => {
    const { simulator, app } = renderHub("/hub");
    await home();
    await userEvent.type(searchField(), "q");
    await delay(600);
    expect(searches(simulator)).toEqual([]);
    await userEvent.type(searchField(), "wen");
    const typedAt = Date.now();
    await screen.findByRole("heading", { name: "Results for “qwen”" });
    expect(searches(simulator)).toHaveLength(1);
    const call = simulator.calls.find((c) => c.command === "hub_search")!;
    expect(call.at - typedAt).toBeGreaterThanOrEqual(380);
    expect(app.router.state.location.search).toEqual({ q: "qwen" });
    await waitFor(() => expect(cardNames()).toContain("Qwen3-8B-Medical"));

    // Résultats non vérifiés : badge et infobulle ; « Verified only » proposé.
    const unverified = within(card("Qwen3-8B-Medical")).getByText("Unverified");
    expect(unverified.getAttribute("title")).toBe(
      "This publisher isn't on ChatonLM's list of trusted publishers. Check the model card before using it.",
    );
    await userEvent.click(chip("Verified only"));
    await waitFor(() =>
      expect(searches(simulator).at(-1)).toMatchObject({
        text: "qwen",
        filters: { ...NO_FILTERS, verifiedOnly: true },
      }),
    );
    await waitFor(() => expect(cardNames()).not.toContain("Qwen3-8B-Medical"));
  });

  it("Load more asks for the next page with the cursor", async () => {
    const { simulator } = renderHub("/hub?q=qwen", ({ hub }) => {
      hub.pageSize = 2;
    });
    await screen.findByRole("heading", { name: "Results for “qwen”" });
    await waitFor(() => expect(cardNames()).toHaveLength(2));
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(cardNames()).toHaveLength(4));
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(cardNames()).toHaveLength(6));
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
    expect(searches(simulator).map((search) => search.cursor)).toEqual([
      null,
      "2",
      "4",
    ]);
  });

  it("says when nothing matches", async () => {
    renderHub("/hub?q=nothing-like-this");
    expect(
      await screen.findByText("No models match your search."),
    ).toBeTruthy();
  });

  it("/ goes to the search field, Escape empties it", async () => {
    const { app } = renderHub("/hub");
    await home();
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    fireEvent.keyDown(document.body, { key: "/" });
    expect(document.activeElement).toBe(searchField());
    // Dans le champ, « / » s'écrit.
    await userEvent.keyboard("gemma/");
    expect(searchField()).toHaveProperty("value", "gemma/");
    await userEvent.keyboard("{Backspace}");
    await screen.findByRole("heading", { name: "Results for “gemma”" });
    await userEvent.keyboard("{Escape}");
    expect(searchField()).toHaveProperty("value", "");
    await home();
    expect(app.router.state.location.search).toEqual({});
  });

  it("the clear button empties the search", async () => {
    renderHub("/hub?q=gemma");
    await screen.findByRole("heading", { name: "Results for “gemma”" });
    await userEvent.click(screen.getByRole("button", { name: "Clear search" }));
    await home();
    expect(searchField()).toHaveProperty("value", "");
  });

  it("the clear button, from the keyboard, leaves the focus in the field", async () => {
    renderHub("/hub?q=gemma");
    await screen.findByRole("heading", { name: "Results for “gemma”" });
    act(() => searchField().focus());
    await userEvent.tab();
    const clear = screen.getByRole("button", { name: "Clear search" });
    expect(document.activeElement).toBe(clear);
    await userEvent.keyboard("{Enter}");
    expect(searchField()).toHaveProperty("value", "");
    expect(screen.queryByRole("button", { name: "Clear search" })).toBeNull();
    expect(document.activeElement).toBe(searchField());
  });
});

describe("Discover: Hugging Face problems", () => {
  it("offline with saved models: banner with the date, Retry", async () => {
    const { simulator, hub } = renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "offline" };
    });
    const banner = await screen.findByTestId("hub-banner");
    expect(banner.textContent).toMatch(
      /^You're offline\. Showing models saved .+ ago\.Retry$/,
    );
    // Les données anciennes restent affichées.
    expect(screen.getByRole("heading", { name: "Trending" })).toBeTruthy();
    hub.problem = null;
    await userEvent.click(
      within(banner).getByRole("button", { name: "Retry" }),
    );
    await waitFor(() => expect(screen.queryByTestId("hub-banner")).toBeNull());
    expect(simulator.callsTo("hub_home")).toHaveLength(2);
  });

  it("offline with nothing saved", async () => {
    renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "offline" };
      hub.cached = false;
    });
    expect(
      await screen.findByRole("heading", { name: "You're offline" }),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Connect to the internet to browse models on Hugging Face. Your installed models are still available.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("Hugging Face not responding, with an incident", async () => {
    renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "unavailable", incident: true };
    });
    const banner = await screen.findByTestId("hub-banner");
    expect(banner.textContent).toMatch(
      /^Hugging Face isn't responding\. Hugging Face reports an incident\. Showing models saved .+\.Retry$/,
    );
  });

  it("Hugging Face not responding, nothing saved", async () => {
    renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "unavailable", incident: false };
      hub.cached = false;
    });
    expect(
      await screen.findByRole("heading", {
        name: "Hugging Face isn't responding",
      }),
    ).toBeTruthy();
    expect(screen.getByText("Try again in a few minutes.")).toBeTruthy();
  });

  it("rate limited: countdown, Retry enabled at the end", async () => {
    renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "rateLimited", retryAfter: 2 };
    });
    const banner = await screen.findByTestId("hub-banner");
    expect(banner.textContent).toContain(
      "Hugging Face is limiting requests. Try again in 2 s.",
    );
    const retry = within(banner).getByRole("button", { name: "Retry" });
    expect(retry).toHaveProperty("disabled", true);
    await waitFor(() =>
      expect(banner.textContent).toContain("Try again in 1 s."),
    );
    await waitFor(() => expect(retry).toHaveProperty("disabled", false));
  });

  it("rate limited again for as long: the countdown starts over", async () => {
    const { simulator } = renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "rateLimited", retryAfter: 1 };
    });
    const banner = () => screen.getByTestId("hub-banner");
    const retry = () => within(banner()).getByRole("button", { name: "Retry" });
    await screen.findByTestId("hub-banner");
    await waitFor(() => expect(retry()).toHaveProperty("disabled", false));
    expect(banner().textContent).toContain("Try again in 0 s.");
    // Même réponse (même attente, mêmes données en cache) : nouvelle limite.
    await userEvent.click(retry());
    await waitFor(() => expect(simulator.callsTo("hub_home")).toHaveLength(2));
    await waitFor(() =>
      expect(banner().textContent).toContain("Try again in 1 s."),
    );
    expect(retry()).toHaveProperty("disabled", true);
  });

  it("rate limited again with nothing saved: the countdown starts over", async () => {
    const { simulator } = renderHub("/hub", ({ hub }) => {
      hub.problem = { kind: "rateLimited", retryAfter: 1 };
      hub.cached = false;
    });
    await screen.findByRole("heading", { name: "Too many requests" });
    const retry = () => screen.getByRole("button", { name: "Retry" });
    await waitFor(() => expect(retry()).toHaveProperty("disabled", false));
    expect(screen.getByText(/Try again in 0 s\./)).toBeTruthy();
    await userEvent.click(retry());
    await waitFor(() => expect(simulator.callsTo("hub_home")).toHaveLength(2));
    expect(await screen.findByText(/Try again in 1 s\./)).toBeTruthy();
    expect(retry()).toHaveProperty("disabled", true);
  });

  it("an internal failure shows its message and Retry", async () => {
    const { hub, simulator } = renderHub("/hub", ({ hub }) => {
      hub.failures.set("hub_home", {
        kind: "internal",
        message: "not implemented yet",
      });
    });
    expect(await screen.findByText("Something went wrong")).toBeTruthy();
    expect(screen.getByText("not implemented yet")).toBeTruthy();
    hub.failures.clear();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await home();
    expect(simulator.callsTo("hub_home")).toHaveLength(2);
  });
});
