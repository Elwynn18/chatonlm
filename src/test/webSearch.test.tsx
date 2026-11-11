// Recherche web côté frontend : de bout en bout, l'app face au simulateur,
// qui rejoue les séquences web-search*.json enregistrées par les tests du
// moteur Rust ; puis les éléments propres à la recherche de ChatonLM, faite
// depuis cet ordinateur (actions sous ses erreurs, bulle du bouton, mention
// « Powered by Brave Search »), et leurs textes dans les trois langues.

import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { ToolCall } from "@/bindings/ToolCall";
import { ToolResult } from "@/components/ToolCalls";
import { WebSearchAttribution } from "@/components/WebSearchAttribution";
import { WebSearchButton } from "@/components/WebSearchButton";
import { WebSearchErrorActions } from "@/components/WebSearchErrorActions";
import { setLanguage } from "@/i18n";
import * as streams from "@/lib/streams";
import {
  isWebSearchError,
  searchProvider,
  type WebSearchErrorKind,
} from "@/lib/webSearch";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp, texts } from "@/test/renderApp";
import { installSimulator, sequence } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  streams.resetStreams();
  cleanup();
});

/** Modèle et question des séquences enregistrées (recherche web). */
const MODEL = sequence("web-search").request.model;

const LATEST = sequence("web-search").request.content;

function setup({ path = "/c/new", recorded = [] as string[] } = {}) {
  const simulator = installSimulator({ latencyMs: 20 });
  const backend = fakeBackend(simulator, {
    selectedModel: MODEL,
    details: [
      {
        model: MODEL,
        capabilities: ["completion", "tools"],
        thinking: null,
        alwaysThinks: false,
      },
    ],
  });
  backend.settings.webSearchEnabled = true;
  for (const name of recorded) backend.record(sequence(name));
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** Tape et envoie, une fois le modèle et la recherche web prêts. */
async function send(text: string) {
  const textarea = await screen.findByLabelText("Message");
  await screen.findByRole("button", { name: "Web search", pressed: true });
  await userEvent.type(textarea, `${text}{Enter}`);
}

const callLines = () =>
  [...document.querySelectorAll("[data-role=tool-calls] > div")].map(
    (line) => line.textContent,
  );
const resultBlocks = () => [
  ...document.querySelectorAll<HTMLElement>("[data-role=tool]"),
];
const copyButtons = () =>
  screen
    .queryAllByRole("button", { name: "Copy" })
    .filter((button) => button.title === "Copy");

/** Réponse finie : flux retiré, bouton d'envoi revenu. */
async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    expect(streams.pendingNewStream()).toBeNull();
    screen.getByLabelText("Send message");
  });
}

describe("web search", () => {
  it("shows the call, its folded result, the dots, then the next answer", async () => {
    const { simulator } = setup();
    const replay = simulator.queue("web-search", { stepwise: true });
    await send(LATEST);
    await replay.started;

    await replay.until("toolCalls");
    await waitFor(() =>
      expect(callLines()).toEqual([`Searching for ${LATEST}…`]),
    );
    expect(resultBlocks()).toEqual([]);

    await replay.until("toolResult");
    await waitFor(() => expect(resultBlocks()).toHaveLength(1));
    const header = within(resultBlocks()[0]).getByRole("button");
    expect(header.textContent).toBe(`Search results for ${LATEST}`);
    expect(header.getAttribute("aria-expanded")).toBe("false");
    // Entre le résultat et le tour suivant : les points.
    await screen.findByTestId("waiting");

    await replay.finish();
    await replyEnded();
    expect(screen.queryByTestId("waiting")).toBeNull();
    // Relu en base : appel, résultat, réponse finale avec son bouton copier.
    expect(callLines()).toEqual([`Searching for ${LATEST}…`]);
    expect(resultBlocks()).toHaveLength(1);
    const [answer] = texts("assistant");
    expect(answer).toMatch(/^Found: \{"results":/);
    expect(texts("assistant")).toHaveLength(1);
    expect(copyButtons()).toHaveLength(1);

    await userEvent.click(within(resultBlocks()[0]).getByRole("button"));
    // Résultat montré : titres et liens, sans les extraits (donnés au seul
    // modèle).
    const shown = resultBlocks()[0].querySelector("pre")?.textContent ?? "";
    expect(shown).toContain(`"title": "DuckDuckGo result 1 for ${LATEST}"`);
    expect(shown).toContain('"url": "https://duckduckgo.example/1"');
    expect(shown).not.toContain("Extrait");
  });

  it("Stop during the tool keeps the call, without result nor dots", async () => {
    const { simulator } = setup();
    const replay = simulator.queue("web-search-cancelled", { stepwise: true });
    const text = sequence("web-search-cancelled").request.content;
    await send(text);
    await replay.started;
    await replay.until("toolCalls");
    await waitFor(() =>
      expect(callLines()).toEqual([`Searching for ${text}…`]),
    );

    await userEvent.click(screen.getByLabelText("Stop"));
    await waitFor(() => expect(replay.cancelRequested).toBe(true));
    await replay.finish();
    await replyEnded();
    expect(callLines()).toEqual([`Searching for ${text}…`]);
    expect(resultBlocks()).toEqual([]);
    expect(screen.queryByTestId("waiting")).toBeNull();
    expect(texts("assistant")).toEqual([]);
  });
});

const GENERATE = "Generate without web search";
const SETTINGS = "Web search settings";

describe("actions under a web search error", () => {
  const kinds: WebSearchErrorKind[] = [
    "webSearchUnavailable",
    "webSearchRefused",
  ];

  it("knows its kinds", () => {
    for (const kind of kinds) expect(isWebSearchError(kind)).toBe(true);
    for (const kind of ["remoteKey", "model", undefined] as (
      ChatErrorKind | undefined
    )[]) {
      expect(isWebSearchError(kind)).toBe(false);
    }
  });

  it("search unavailable: Retry with the search, or without it", async () => {
    const onRetry = vi.fn();
    const onOpenSettings = vi.fn();
    render(
      <WebSearchErrorActions
        kind="webSearchUnavailable"
        onRetry={onRetry}
        onOpenSettings={onOpenSettings}
      />,
    );
    // Rien à régler : pas de bouton des réglages.
    expect(screen.queryByRole("button", { name: SETTINGS })).toBeNull();
    const buttons = screen.getAllByRole("button").map((b) => b.textContent);
    expect(buttons).toEqual(["Retry", GENERATE]);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("button", { name: GENERATE }));
    expect(onRetry.mock.calls).toEqual([[true], [false]]);
  });

  it("search refused: Web search settings first, then Retry or without", async () => {
    const onRetry = vi.fn();
    const onOpenSettings = vi.fn();
    render(
      <WebSearchErrorActions
        kind="webSearchRefused"
        onRetry={onRetry}
        onOpenSettings={onOpenSettings}
      />,
    );
    const buttons = screen.getAllByRole("button").map((b) => b.textContent);
    expect(buttons).toEqual([SETTINGS, "Retry", GENERATE]);
    await userEvent.click(screen.getByRole("button", { name: SETTINGS }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: GENERATE }));
    expect(onRetry).toHaveBeenCalledExactlyOnceWith(false);
  });

  it("nothing to send again (new chat): no button, none disabled forgotten", async () => {
    const { container } = render(
      <WebSearchErrorActions kind="webSearchUnavailable" />,
    );
    expect(container.querySelectorAll("button")).toHaveLength(0);
    cleanup();
    const onRetry = vi.fn();
    render(
      <WebSearchErrorActions
        kind="webSearchUnavailable"
        onRetry={onRetry}
        retryDisabled
      />,
    );
    for (const button of screen.getAllByRole("button")) {
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
  });
});

describe("the web search button", () => {
  it("says what a click does, and asks the engine nothing", () => {
    const simulator = installSimulator();
    const { rerender } = render(
      <WebSearchButton isVisible isActive={false} onToggle={() => {}} />,
    );
    expect(screen.getByRole("button", { name: "Web search" }).title).toBe(
      "Enable web search",
    );
    rerender(<WebSearchButton isVisible isActive onToggle={() => {}} />);
    expect(screen.getByRole("button", { name: "Web search" }).title).toBe(
      "Disable web search",
    );
    expect(simulator.calls).toHaveLength(0);
  });
});

/** Appel `web_search` terminé avec ce résultat enregistré. */
function searchCall(result: string | null, name = "web_search"): ToolCall {
  return {
    name,
    arguments: '{"query":"chaton"}',
    result,
    error: null,
  } as ToolCall;
}

describe("Powered by Brave Search", () => {
  it("is said under Brave's results only", () => {
    const brave = searchCall(
      '{"results":[{"title":"T","url":"https://example.org/1"}],"provider":"brave"}',
    );
    expect(searchProvider(brave)).toBe("brave");
    render(<WebSearchAttribution call={brave} />);
    expect(screen.getByTestId("web-search-attribution").textContent).toBe(
      "Powered by Brave Search · About web search",
    );
    cleanup();
    // Autre moteur (DuckDuckGo, Mojeek…) : le lien seul.
    render(
      <WebSearchAttribution
        call={searchCall('{"results":[],"provider":"duckduckgo"}')}
      />,
    );
    expect(screen.getByTestId("web-search-attribution").textContent).toBe(
      "About web search",
    );
    cleanup();
    for (const call of [
      searchCall('{"results":[]}'),
      searchCall("not json"),
      searchCall(null),
      searchCall('{"provider":"brave"}', "web_fetch"),
    ]) {
      const { container } = render(<WebSearchAttribution call={call} />);
      expect(container.textContent).toBe("");
      cleanup();
    }
  });

  it("sits under the block of the results, folded or not", async () => {
    render(
      <ToolResult
        call={searchCall(
          '{"results":[{"title":"T","url":"https://example.org/1"}],"provider":"brave"}',
        )}
      />,
    );
    const mention = screen.getByTestId("web-search-attribution");
    const header = screen.getByRole("button");
    expect(header.getAttribute("aria-expanded")).toBe("false");
    // Visible même replié : hors du corps du bloc, après lui.
    expect(mention.closest("[data-folded]")).toBeNull();
    expect(
      header.compareDocumentPosition(mention) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    await userEvent.click(header);
    expect(screen.getByTestId("web-search-attribution")).toBe(mention);
    cleanup();
    render(<ToolResult call={searchCall('{"results":[]}')} />);
    expect(screen.queryByTestId("web-search-attribution")).toBeNull();
  });

  it("links to the web search page of the site, in the app's language", async () => {
    const simulator = installSimulator();
    simulator.handle("open_external", () => null);
    render(
      <WebSearchAttribution
        call={searchCall('{"results":[],"provider":"brave"}')}
      />,
    );
    const link = screen.getByRole("link", { name: "About web search" });
    expect(link.getAttribute("href")).toBe(
      "https://chatonlm.com/en/web-search/",
    );
    await userEvent.click(link);
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toHaveLength(1),
    );
    cleanup();
    act(() => setLanguage("fr"));
    render(
      <WebSearchAttribution
        call={searchCall('{"results":[],"provider":"brave"}')}
      />,
    );
    expect(
      screen
        .getByRole("link", { name: "À propos de la recherche web" })
        .getAttribute("href"),
    ).toBe("https://chatonlm.com/fr/web-search/");
    act(() => setLanguage("en"));
  });
});

describe("texts of the web search", () => {
  it("are translated", () => {
    usePseudoLanguage();
    installSimulator();
    render(
      <>
        <WebSearchButton isVisible isActive onToggle={() => {}} />
        <WebSearchErrorActions
          kind="webSearchRefused"
          onRetry={() => {}}
          onOpenSettings={() => {}}
        />
        <WebSearchAttribution
          call={searchCall('{"results":[],"provider":"brave"}')}
        />
      </>,
    );
    expect(untranslatedTexts(document.body)).toEqual([]);
  });

  it("in French", () => {
    act(() => setLanguage("fr"));
    render(
      <WebSearchErrorActions
        kind="webSearchRefused"
        onRetry={() => {}}
        onOpenSettings={() => {}}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Paramètres de la recherche web" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Générer sans recherche web" }),
    ).toBeTruthy();
  });
});
